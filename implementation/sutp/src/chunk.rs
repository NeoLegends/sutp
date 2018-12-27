//! This module implements the data format of chunks as specified in
//! https://laboratory.comsys.rwth-aachen.de/sutp/data-format/blob/master/README.md.

use byteorder::{NetworkEndian, WriteBytesExt};
use bytes::{Buf, Bytes, IntoBuf};
use log::warn;
use std::{
    io::{self, Result, Write},
    mem, u16,
};

/// A list of supported compression algorithms in order of preference.
pub const SUPPORTED_COMPRESSION_ALGS: [CompressionAlgorithm; 2] =
    [CompressionAlgorithm::Snappy, CompressionAlgorithm::Gzip];

/// An SUTP chunk.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum Chunk {
    /// The chunk is a payload chunk containing data to be transferred.
    Payload(Bytes),

    /// SYN chunk.
    Syn,

    /// FIN chunk.
    Fin,

    /// ABRT chunk.
    Abort,

    /// SACK chunk.
    ///
    /// Contains the ACK number and the NAK list.
    Sack(u32, Vec<u32>),

    /// Compression negotiation chunk.
    ///
    /// Contains a list of compression algorithms.
    CompressionNegotiation(Vec<CompressionAlgorithm>),

    /// Security flag chunk.
    ///
    /// The flag is true if the segment is insecure.
    SecurityFlag(bool),

    /// Unknown chunk with arbitrary data.
    ///
    /// The first value is the type, the second value the payload data.
    Unknown(u16, Bytes),
}

/// The type of compression algorithm to be applied.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub enum CompressionAlgorithm {
    Gzip,
    Snappy,
    Unknown(u32),
}

const U16_SIZE: usize = mem::size_of::<u16>();
const U32_SIZE: usize = mem::size_of::<u32>();
const ZEROS: [u8; 3] = [0; 3];

/// The constant overhead of serializing a single chunk.
const BINARY_OVERHEAD: usize = 2 * U16_SIZE;

#[allow(dead_code)]
impl Chunk {
    /// Calculates the length of the chunk in its binary serialized form.
    pub fn binary_len(&self) -> usize {
        let payload_size = match self {
            Chunk::Abort | Chunk::Fin | Chunk::Syn => 0,
            Chunk::CompressionNegotiation(algs) => algs.len() * U32_SIZE,
            Chunk::Unknown(_, data) | Chunk::Payload(data) => {
                data.len() + Self::calculate_padding(data.len())
            }
            Chunk::Sack(_, list) => U32_SIZE + list.len() * U32_SIZE,
            Chunk::SecurityFlag(_) => U32_SIZE,
        };

        payload_size + BINARY_OVERHEAD
    }

    /// Returns whether the chunk is an ABRT chunk.
    pub fn is_abrt(&self) -> bool {
        match self {
            Chunk::Abort => true,
            _ => false,
        }
    }

    /// Returns whether the chunk is a compression negotiation chunk.
    pub fn is_compression_negotiation(&self) -> bool {
        match self {
            Chunk::CompressionNegotiation(_) => true,
            _ => false,
        }
    }

    /// Returns whether the chunk is a FIN chunk.
    pub fn is_fin(&self) -> bool {
        match self {
            Chunk::Fin => true,
            _ => false,
        }
    }

    /// Returns whether the chunk is a payload chunk.
    pub fn is_payload(&self) -> bool {
        match self {
            Chunk::Payload(_) => true,
            _ => false,
        }
    }

    /// Returns whether the chunk is a SACK chunk.
    pub fn is_sack(&self) -> bool {
        match self {
            Chunk::Sack(_, _) => true,
            _ => false,
        }
    }

    /// Returns whether the chunk is a security flag chunk.
    pub fn is_security_flag(&self) -> bool {
        match self {
            Chunk::SecurityFlag(_) => true,
            _ => false,
        }
    }

    /// Returns whether the chunk is a SYN chunk.
    pub fn is_syn(&self) -> bool {
        match self {
            Chunk::Syn => true,
            _ => false,
        }
    }

    /// Returns whether the chunk is an unknown chunk.
    pub fn is_unknown(&self) -> bool {
        match self {
            Chunk::Unknown(_, _) => true,
            _ => false,
        }
    }
}

impl Chunk {
    /// Reads a chunk from the given reader.
    ///
    /// It is strongly advised to pass a buffering `io.Read` implementation
    /// since the parser will issue lots of small calls to `read`.
    ///
    /// The function returns `None`, when the reader has gone EOF while parsing
    /// the chunk type. This usually indicates that we have reached the end of the
    /// chunk list, and not an unexpected EOF. Otherwise returns `Some`.
    pub fn read_from(r: &mut Bytes) -> Result<Option<Self>> {
        let mut buf = r.as_ref().into_buf();
        if buf.remaining() < U32_SIZE {
            return Ok(None);
        }

        let ty = buf.get_u16_be();

        // Advancing the buf doesn't advance the Bytes
        r.advance(U16_SIZE);

        // Type list as specified at https://laboratory.comsys.rwth-aachen.de/sutp/data-format
        let (variant, bytes_read) = match ty {
            0x0 => Self::read_payload(r),
            0x1 => Self::read_syn(r),
            0x2 => Self::read_fin(r),
            0x3 => Self::read_abrt(r),
            0x4 => Self::read_sack(r),
            0xa0 => Self::read_compression_negotiation(r),
            0xfe => Self::read_security_flag(r),
            x => Self::read_unknown(x, r),
        }?;

        // Discard padding between chunks
        let padding = Self::calculate_padding(bytes_read);
        if r.len() < padding {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }

        r.advance(padding);

        Ok(Some(variant))
    }

    /// Writes the chunk to the given writer.
    ///
    /// It is strongly advised to pass a buffering `io.Write` implementation
    /// since the serializer will issue lots of small calls to `write`.
    pub fn write_to(&self, w: &mut impl Write) -> Result<()> {
        let payload_length = match self {
            Chunk::Abort => Self::write_abrt(w),
            Chunk::CompressionNegotiation(list) => {
                Self::write_compression_negotiation(list, w)
            }
            Chunk::Fin => Self::write_fin(w),
            Chunk::Payload(data) => Self::write_payload(data, w),
            Chunk::Sack(ack_no, nak_list) => Self::write_sack(*ack_no, nak_list, w),
            Chunk::SecurityFlag(is_insecure) => {
                Self::write_security_flag(*is_insecure, w)
            }
            Chunk::Syn => Self::write_syn(w),
            Chunk::Unknown(ty, data) => Self::write_unknown(*ty, data, w),
        }?;

        // Write padding as necessary
        let padding = Self::calculate_padding(payload_length);
        w.write_all(&ZEROS[..padding])?;

        Ok(())
    }

    /// Calculates the required padding when given a chunk payload of the given size.
    fn calculate_padding(payload_length: usize) -> usize {
        (4 - (payload_length % 4)) % 4
    }
}

// Read implementations
impl Chunk {
    /// Reads an ABRT chunk.
    fn read_abrt(r: &mut Bytes) -> Result<(Self, usize)> {
        Self::read_flag_chunk(Chunk::Abort, r)
    }

    /// Reads an SUTP compression negotiation chunk.
    fn read_compression_negotiation(r: &mut Bytes) -> Result<(Self, usize)> {
        let mut buf = r.as_ref().into_buf();
        let len = buf.get_u16_be() as usize;

        // Ensure the list length is valid (i. e. a multiple of size_of::<u32>())
        if (len % U32_SIZE) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "compression negotiation list length not multiple of {}",
                    U32_SIZE
                ),
            ));
        }

        assert_size!(buf, len);

        // You can collect an iterator of results into a result of an iterator
        let list = (0..(len / U32_SIZE))
            .map(|_| buf.get_u32_be().into())
            .collect();

        r.advance(U16_SIZE + len);

        Ok((Chunk::CompressionNegotiation(list), len))
    }

    /// Reads a FIN chunk.
    fn read_fin(r: &mut Bytes) -> Result<(Self, usize)> {
        Self::read_flag_chunk(Chunk::Fin, r)
    }

    /// Reads a payload chunk.
    ///
    /// The implementation is based on read_unknown and just
    /// changes the type of the chunk that was read.
    fn read_payload(r: &mut Bytes) -> Result<(Self, usize)> {
        Self::read_unknown(0, r).map(|(ch, len)| match ch {
            Chunk::Unknown(_, data) => (Chunk::Payload(data), len),
            _ => unreachable!("expected `Unknown` variant"),
        })
    }

    /// Reads a SACK chunk.
    fn read_sack(r: &mut Bytes) -> Result<(Self, usize)> {
        let mut buf = r.as_ref().into_buf();
        let len = buf.get_u16_be() as usize;

        if (len % U32_SIZE) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("sack list length is not a multiple of {}", U32_SIZE),
            ));
        }

        assert_size!(buf, len);

        let ack_no = buf.get_u32_be();
        let nak_list = (0..(len.saturating_sub(U32_SIZE) / U32_SIZE))
            .map(|_| buf.get_u32_be())
            .collect();

        r.advance(U16_SIZE + len);

        Ok((Chunk::Sack(ack_no, nak_list), len))
    }

    /// Reads a security flag chunk.
    fn read_security_flag(r: &mut Bytes) -> Result<(Self, usize)> {
        let mut buf = r.as_ref().into_buf();
        let len = buf.get_u16_be() as usize;

        if len != 1 {
            warn!(
                "security flag chunk length was {}, but is expected to be 1",
                len
            );
        }
        assert_size!(buf, 1);

        let flag_value = buf.get_u8();

        r.advance(U16_SIZE + len);

        Ok((Chunk::SecurityFlag(flag_value != 0), 1))
    }

    /// Reads a SYN chunk.
    fn read_syn(r: &mut Bytes) -> Result<(Self, usize)> {
        Self::read_flag_chunk(Chunk::Syn, r)
    }

    /// Reads the data of an unknown chunk into a buffer.
    fn read_unknown(ty: u16, r: &mut Bytes) -> Result<(Self, usize)> {
        let mut buf = r.as_ref().into_buf();
        let len = buf.get_u16_be() as usize;

        r.advance(U16_SIZE);
        if r.len() < len {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }

        Ok((Chunk::Unknown(ty, r.split_to(len)), len))
    }

    /// Reads a flag (zero-sized) chunk from the reader.
    fn read_flag_chunk(ch: Chunk, r: &mut Bytes) -> Result<(Self, usize)> {
        let mut buf = r.as_ref().into_buf();
        let len = buf.get_u16_be() as usize;

        // Ensure we are given correct data, but otherwise discard what we've been
        // given for a robust implementation.
        if len != 0 {
            warn!("flag chunk length is {}, but is expected to be 0", len);
        }
        assert_size!(buf, len);

        // Reading from the buffer doesn't advance the Bytes
        r.advance(U16_SIZE + len);

        Ok((ch, len))
    }
}

// Write implementations
impl Chunk {
    /// Writes an ABRT chunk to the given writer.
    fn write_abrt(w: &mut impl Write) -> Result<usize> {
        Self::write_chunk_header(0x3, 0, w)?;
        Ok(0)
    }

    /// Writes the compression negotiation chunk to the given writer.
    fn write_compression_negotiation(
        list: &[CompressionAlgorithm],
        w: &mut impl Write,
    ) -> Result<usize> {
        let payload_size = list.len() * U32_SIZE;
        assert!(payload_size <= u16::MAX as usize);

        Self::write_chunk_header(0xa0, payload_size as u16, w)?;

        for &alg in list {
            w.write_u32::<NetworkEndian>(alg.into())?;
        }

        Ok(payload_size)
    }

    /// Writes a FIN chunk to the given writer.
    fn write_fin(w: &mut impl Write) -> Result<usize> {
        Self::write_chunk_header(0x2, 0, w)?;
        Ok(0)
    }

    /// Writes a payload chunk containing the given data to the given writer.
    fn write_payload(data: &[u8], w: &mut impl Write) -> Result<usize> {
        assert!(data.len() <= u16::MAX as usize);

        Self::write_chunk_header(0x0, data.len() as u16, w)?;
        w.write_all(&data)?;

        Ok(data.len())
    }

    /// Writes a SACK chunk containing the given data to the given writer.
    fn write_sack(
        ack_no: u32,
        nak_list: &[u32],
        w: &mut impl Write,
    ) -> Result<usize> {
        let payload_len = U32_SIZE + nak_list.len() * U32_SIZE;

        assert!(payload_len <= u16::MAX as usize);
        Self::write_chunk_header(0x4, payload_len as u16, w)?;

        w.write_u32::<NetworkEndian>(ack_no)?;

        for &nak_no in nak_list {
            w.write_u32::<NetworkEndian>(nak_no)?;
        }

        Ok(payload_len)
    }

    /// Writes the security flag chunk to the given writer.
    fn write_security_flag(is_insecure: bool, w: &mut impl Write) -> Result<usize> {
        Self::write_chunk_header(0xfe, 1, w)?;
        w.write_u8(if is_insecure { 1 } else { 0 })?;

        Ok(1)
    }

    /// Writes a SYN to the given writer.
    fn write_syn(w: &mut impl Write) -> Result<usize> {
        Self::write_chunk_header(0x1, 0, w)?;
        Ok(0)
    }

    /// Writes an unknown chunk of the given type.
    fn write_unknown(ty: u16, data: &[u8], w: &mut impl Write) -> Result<usize> {
        assert!(data.len() <= u16::MAX as usize);

        Self::write_chunk_header(ty, data.len() as u16, w)?;
        w.write_all(data)?;

        Ok(data.len())
    }

    /// Writes chunk type and payload length to the given writer.
    fn write_chunk_header(ty: u16, len: u16, w: &mut impl Write) -> Result<()> {
        w.write_u16::<NetworkEndian>(ty)?;
        w.write_u16::<NetworkEndian>(len)?;

        Ok(())
    }
}

impl CompressionAlgorithm {
    /// Creates a chunk containing this compression algorithm.
    pub fn into_chunk(self) -> Chunk {
        Chunk::CompressionNegotiation(vec![self])
    }

    /// Returns whether the algorithm is known.
    pub fn is_known(self) -> bool {
        match self {
            CompressionAlgorithm::Unknown(_) => false,
            _ => true,
        }
    }
}

impl From<u32> for CompressionAlgorithm {
    fn from(val: u32) -> Self {
        match val {
            0x1 => CompressionAlgorithm::Gzip,
            0x4 => CompressionAlgorithm::Snappy,
            x => CompressionAlgorithm::Unknown(x),
        }
    }
}

impl From<CompressionAlgorithm> for u32 {
    fn from(alg: CompressionAlgorithm) -> Self {
        match alg {
            CompressionAlgorithm::Gzip => 0x1,
            CompressionAlgorithm::Snappy => 0x4,
            CompressionAlgorithm::Unknown(x) => x,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn binary_len() {
        assert_eq!(BINARY_OVERHEAD, Chunk::Abort.binary_len());
        assert_eq!(BINARY_OVERHEAD, Chunk::Fin.binary_len());
        assert_eq!(BINARY_OVERHEAD, Chunk::Syn.binary_len());

        assert_eq!(
            BINARY_OVERHEAD + 4,
            Chunk::Payload(vec![1].into()).binary_len(),
        );
        assert_eq!(
            BINARY_OVERHEAD + 4,
            Chunk::Unknown(10, vec![1].into()).binary_len(),
        );
        assert_eq!(
            BINARY_OVERHEAD + 24,
            Chunk::Sack(14, vec![1, 2, 3, 4, 5].into()).binary_len(),
        );
    }

    #[test]
    fn deserialize_compression_negotiation() {
        let mut data = vec![
            0x0, 0xa0, 0x0, 0xc, 0x0, 0x0, 0x0, 0x1, 0x0, 0x0, 0x0, 0xa, 0x0, 0x0,
            0x0, 0x4,
        ]
        .into();

        assert_eq!(
            Chunk::read_from(&mut data).unwrap().unwrap(),
            Chunk::CompressionNegotiation(vec![
                CompressionAlgorithm::Gzip,
                CompressionAlgorithm::Unknown(10),
                CompressionAlgorithm::Snappy,
            ]),
        );
    }

    #[test]
    fn deserialize_compression_negotiation_zero_length() {
        let mut data = vec![0x0, 0xa0, 0x0, 0x0].into();
        assert_eq!(
            Chunk::read_from(&mut data).unwrap().unwrap(),
            Chunk::CompressionNegotiation(Vec::new()),
        );
    }

    /// Test deserialization of flag chunks.
    #[test]
    fn deserialize_flag_chunks() {
        fn deserialize_flag_chunk(ty: u8, should: Chunk) {
            // Check the base case
            let mut data = vec![0x0, ty, 0x0, 0x0].into();
            assert_eq!(Chunk::read_from(&mut data).unwrap().unwrap(), should,);

            // Now check if we correctly parse (technically) incorrect lengths.
            // The following should all parse equivalent, since we're padding
            // to multiples of 32 bits.
            let data = vec![
                vec![0x0, ty, 0x0, 0x1, 0x0, 0x0, 0x0, 0x0].into(),
                vec![0x0, ty, 0x0, 0x2, 0x0, 0x0, 0x0, 0x0].into(),
                vec![0x0, ty, 0x0, 0x3, 0x0, 0x0, 0x0, 0x0].into(),
                vec![0x0, ty, 0x0, 0x4, 0x0, 0x0, 0x0, 0x0].into(),
            ];
            for mut cur in data.into_iter() {
                assert_eq!(Chunk::read_from(&mut cur).unwrap().unwrap(), should,);
            }
        }

        deserialize_flag_chunk(0x1, Chunk::Syn);
        deserialize_flag_chunk(0x2, Chunk::Fin);
        deserialize_flag_chunk(0x3, Chunk::Abort);
    }

    #[test]
    fn deserialize_payload() {
        let mut data = vec![0x0, 0x0, 0x0, 0x4, 0x1, 0x2, 0x3, 0x4].into();

        assert_eq!(
            Chunk::read_from(&mut data).unwrap().unwrap(),
            Chunk::Payload(vec![0x1, 0x2, 0x3, 0x4].into()),
        );
    }

    #[test]
    fn deserialize_payload_empty() {
        let mut data = vec![0x0, 0x0, 0x0, 0x0].into();

        assert_eq!(
            Chunk::read_from(&mut data).unwrap().unwrap(),
            Chunk::Payload(Bytes::new()),
        );
    }

    #[test]
    fn deserialize_sack_chunk() {
        let mut data = vec![
            0x0, 0x4, 0x0, 0xc, 0x0, 0x0, 0x0, 0x4, 0x0, 0x0, 0x0, 0x6, 0x0, 0x0,
            0x0, 0x5,
        ]
        .into();

        let chunk = Chunk::read_from(&mut data).unwrap().unwrap();
        let expected = Chunk::Sack(4, vec![6, 5]);

        assert_eq!(chunk, expected);
    }

    #[test]
    #[should_panic]
    fn deserialize_sack_chunk_zero_length() {
        let mut data = vec![0x0, 0x4, 0x0, 0x0].into();

        Chunk::read_from(&mut data).unwrap().unwrap();
    }

    #[test]
    #[should_panic]
    fn deserialize_sack_chunk_invalid_length() {
        let mut data = vec![0x0, 0x4, 0x0, 0x4].into();

        Chunk::read_from(&mut data).unwrap().unwrap();
    }

    #[test]
    fn deserialize_sack_chunk_empty_nak_list() {
        let mut data = vec![0x0, 0x4, 0x0, 0x4, 0x0, 0x0, 0x0, 0x4].into();

        let chunk = Chunk::read_from(&mut data).unwrap().unwrap();
        let expected = Chunk::Sack(4, Vec::new());

        assert_eq!(chunk, expected);
    }

    #[test]
    fn serialize_compression_negotiation() {
        let mut buf = Vec::new();
        let algorithms =
            vec![CompressionAlgorithm::Snappy, CompressionAlgorithm::Gzip];

        Chunk::CompressionNegotiation(algorithms)
            .write_to(&mut buf)
            .unwrap();

        let expected =
            &[0x0, 0xa0, 0x0, 0x8, 0x0, 0x0, 0x0, 0x4, 0x0, 0x0, 0x0, 0x1];
        assert_eq!(&buf, expected);
    }

    #[test]
    fn serialize_compression_negotiation_empty() {
        let mut buf = Vec::new();
        Chunk::CompressionNegotiation(Vec::new())
            .write_to(&mut buf)
            .unwrap();

        let expected = &[0x0, 0xa0, 0x0, 0x0];
        assert_eq!(&buf, expected);
    }

    /// Test serialization of flag chunks.
    #[test]
    fn serialize_flag_chunks() {
        fn serialize_flag_chunk(ch: Chunk, expected_type: u8) {
            let mut buf = Cursor::new(vec![]);
            ch.write_to(&mut buf).unwrap();

            assert_eq!(&buf.into_inner(), &[0x0, expected_type, 0x0, 0x0],);
        }

        serialize_flag_chunk(Chunk::Syn, 0x1);
        serialize_flag_chunk(Chunk::Fin, 0x2);
        serialize_flag_chunk(Chunk::Abort, 0x3);
    }

    #[test]
    fn serialize_payload() {
        let mut buf = Vec::new();
        let payload = vec![0x0, 0x0, 0x1, 0x1].into();

        Chunk::Payload(payload).write_to(&mut buf).unwrap();

        let expected = &[0x0, 0x0, 0x0, 0x4, 0x0, 0x0, 0x1, 0x1];
        assert_eq!(&buf, expected);
    }

    #[test]
    fn serialize_payload_empty() {
        let mut buf = Vec::new();

        Chunk::Payload(Bytes::new()).write_to(&mut buf).unwrap();

        let expected = &[0x0, 0x0, 0x0, 0x0];
        assert_eq!(&buf, expected);
    }

    #[test]
    fn serialize_sack_chunk() {
        let chunk = Chunk::Sack(4, vec![5, 6, 7]);

        let mut buf = Vec::new();
        chunk.write_to(&mut buf).unwrap();

        let expected = &[
            0x0, 0x4, 0x0, 0x10, 0x0, 0x0, 0x0, 0x4, 0x0, 0x0, 0x0, 0x5, 0x0, 0x0,
            0x0, 0x6, 0x0, 0x0, 0x0, 0x7,
        ];
        assert_eq!(&buf, expected);
    }

    #[test]
    fn serialize_sack_chunk_empty_nak_list() {
        let chunk = Chunk::Sack(17, Vec::new());

        let mut buf = Vec::new();
        chunk.write_to(&mut buf).unwrap();

        let expected = &[0x0, 0x4, 0x0, 0x4, 0x0, 0x0, 0x0, 0x11];
        assert_eq!(&buf, expected);
    }

    /// Ensure we cannot read data from nothing.
    #[test]
    fn zero_input() {
        let mut data = vec![].into();
        assert!(Chunk::read_from(&mut data).unwrap().is_none());
    }
}
