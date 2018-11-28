use byteorder::{NetworkEndian, ReadBytesExt, WriteBytesExt};
use std::io::{self, Read, Result, Write};
use std::mem;
use std::u16;

macro_rules! debug_log_assert {
    ($a:expr) => {
        if !$a {
            debug!("assertion failed: {} was {}", stringify!($a), $a);
        }
    };
    ($a:expr, $msg:expr) => {
        if !$a {
            debug!($msg, stringify!($a), $a);
        }
    };
}

macro_rules! debug_log_eq {
    ($a:expr, $b:expr) => {
        if $a != $b {
            debug!("assertion failed: {} != {}", $a, $b);
        }
    };
    ($a:expr, $b:expr, $msg:expr) => {
        if $a != $b {
            debug!($msg, $a, $b);
        }
    };
}

/// An SUTP chunk.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum Chunk {
    /// The chunk is a payload chunk containing data to be transferred.
    Payload(Vec<u8>),

    /// SYN chunk.
    Syn,

    /// FIN chunk.
    Fin,

    /// ABRT chunk.
    Abort,

    /// SACK chunk.
    Sack {
        ack_no: u32,
        nak_list: Vec<u32>,
    },

    /// Compression negotiation chunk.
    CompressionNegotiation(Vec<CompressionAlgorithm>),

    /// Security flag chunk.
    ///
    /// The flag is true if the segment is insecure.
    SecurityFlag(bool),

    /// Unknown chunk with arbitrary data.
    Unknown(u16, Vec<u8>),
}

/// The type of compression algorithm to be applied.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub enum CompressionAlgorithm {
    Gzip,
    Snappy,
    Unknown(u32),
}

/// An SUTP segment.
#[derive(Clone, Debug, Default, Hash, Eq, PartialEq)]
pub struct Segment {
    pub chunks: Vec<Chunk>,
    pub seq_no: u32,
    pub window_size: u32,
}

const U32_SIZE: u16 = mem::size_of::<u32>() as u16;
const ZEROS: [u8; 3] = [0; 3];

impl Chunk {
    /// Reads a chunk from the given reader.
    ///
    /// It is strongly advised to pass a buffering `io.Read` implementation
    /// since the parser will issue lots of small calls to `read`.
    ///
    /// The function returns `None`, when the reader has gone EOF while parsing
    /// the chunk type. This usually indicates that we have reached the end of the
    /// chunk list, and not an unexpected EOF. Otherwise returns `Some`.
    pub fn read_from(r: &mut impl Read) -> Result<Option<Self>> {
        let ty = match r.read_u16::<NetworkEndian>() {
            Ok(ty) => ty,
            Err(ref err) if err.kind() == io::ErrorKind::UnexpectedEof =>
                return Ok(None),
            Err(err) => return Err(err),
        };

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
        let padding = Self::calculate_padding(bytes_read as usize);
        Self::discard_exact(r, padding as u64)?;

        Ok(Some(variant))
    }

    /// Writes the chunk to the given writer.
    ///
    /// It is strongly advised to pass a buffering `io.Write` implementation
    /// since the serializer will issue lots of small calls to `write`.
    pub fn write_to(&self, w: &mut impl Write) -> Result<()> {
        let payload_length = match self {
            Chunk::Abort => Self::write_abrt(w),
            Chunk::CompressionNegotiation(ref list) =>
                Self::write_compression_negotiation(list, w),
            Chunk::Fin => Self::write_fin(w),
            Chunk::Payload(ref data) => Self::write_payload(data, w),
            Chunk::Sack { ref ack_no, ref nak_list } =>
                Self::write_sack(*ack_no, nak_list, w),
            Chunk::SecurityFlag(is_insecure) =>
                Self::write_security_flag(*is_insecure, w),
            Chunk::Syn => Self::write_syn(w),
            Chunk::Unknown(ty, data) => Self::write_unknown(*ty, data, w),
        }?;

        // Write padding as necessary
        let padding = Self::calculate_padding(payload_length as usize);
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
    fn read_abrt(r: &mut impl Read) -> Result<(Self, u16)> {
        Self::read_flag_chunk(Chunk::Abort, r)
    }

    /// Reads an SUTP compression negotiation chunk.
    fn read_compression_negotiation(r: &mut impl Read) -> Result<(Self, u16)> {
        let len = r.read_u16::<NetworkEndian>()?;

        // Ensure the list length is valid (i. e. a multiple of size_of::<u32>())
        debug_log_eq!(len % U32_SIZE, 0);

        // You can collect an iterator of results into a result of an iterator
        let list = (0..(len / U32_SIZE))
            .into_iter()
            .map(|_| r.read_u32::<NetworkEndian>())
            .map(|code_result| code_result.map(|code| code.into()))
            .collect::<Result<_>>()?;
        Ok((Chunk::CompressionNegotiation(list), len))
    }

    /// Reads a FIN chunk.
    fn read_fin(r: &mut impl Read) -> Result<(Self, u16)> {
        Self::read_flag_chunk(Chunk::Fin, r)
    }

    /// Reads a payload chunk.
    ///
    /// The implementation is based on read_unknown and just
    /// changes the type of the chunk that was read.
    fn read_payload(r: &mut impl Read) -> Result<(Self, u16)> {
        Self::read_unknown(0, r)
            .map(|(ch, len)| match ch {
                Chunk::Unknown(_, data) => (Chunk::Payload(data), len),
                _ => unreachable!("expected `Unknown` variant"),
            })
    }

    /// Reads a SACK chunk.
    fn read_sack(r: &mut impl Read) -> Result<(Self, u16)> {
        let len = r.read_u16::<NetworkEndian>()?;

        debug_log_assert!(len >= U32_SIZE);
        debug_log_eq!(len % U32_SIZE, 0);

        let ack_no = r.read_u32::<NetworkEndian>()?;
        let nak_list = (0..((len - U32_SIZE) / U32_SIZE))
            .into_iter()
            .map(|_| r.read_u32::<NetworkEndian>())
            .collect::<Result<_>>()?;

        let chunk = Chunk::Sack {
            ack_no: ack_no,
            nak_list: nak_list,
        };

        Ok((chunk, len))
    }

    /// Reads a security flag chunk.
    fn read_security_flag(r: &mut impl Read) -> Result<(Self, u16)> {
        let len = r.read_u16::<NetworkEndian>()?;

        debug_log_eq!(len, 1);

        let flag_value = r.read_u8()?;
        Ok((Chunk::SecurityFlag(flag_value != 0), 1))
    }

    /// Reads a SYN chunk.
    fn read_syn(r: &mut impl Read) -> Result<(Self, u16)> {
        Self::read_flag_chunk(Chunk::Syn, r)
    }

    /// Reads the data of an unknown chunk into a buffer.
    fn read_unknown(ty: u16, r: &mut impl Read) -> Result<(Self, u16)> {
        let len = r.read_u16::<NetworkEndian>()?;

        let mut buf = vec![0u8; len as usize];
        r.read_exact(buf.as_mut())?;

        Ok((Chunk::Unknown(ty, buf), len))
    }

    /// Reads a flag (zero-sized) chunk from the reader.
    fn read_flag_chunk(ch: Chunk, r: &mut impl Read) -> Result<(Self, u16)> {
        let len = r.read_u16::<NetworkEndian>()?;

        // Ensure we are given correct data, but otherwise discard what we've been
        // given for a robust implementation.
        debug_log_eq!(len, 0);
        Self::discard_exact(r, len as u64)?;

        Ok((ch, len))
    }

    /// Discards exactly `bytes` bytes from the given reader and errors
    /// in any other case.
    fn discard_exact(r: &mut impl Read, bytes: u64) -> Result<()> {
        if bytes == 0 {
            return Ok(())
        }

        // Copy the right amount of bytes to "/dev/null"
        let copied = io::copy(&mut r.take(bytes), &mut io::sink())?;
        if copied < bytes {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }

        Ok(())
    }
}

// Write implementations
impl Chunk {
    /// Writes an ABRT chunk to the given writer.
    fn write_abrt(w: &mut impl Write) -> Result<u16> {
        Self::write_chunk_header(0x3, 0, w)?;
        Ok(0)
    }

    /// Writes the compression negotiation chunk to the given writer.
    fn write_compression_negotiation(
        list: &[CompressionAlgorithm],
        w: &mut impl Write,
    ) -> Result<u16> {
        let payload_size = list.len() * (U32_SIZE as usize);
        assert!(payload_size <= u16::MAX as usize);

        for &alg in list {
            w.write_u32::<NetworkEndian>(alg.into())?;
        }

        Ok(payload_size as u16)
    }

    /// Writes a FIN chunk to the given writer.
    fn write_fin(w: &mut impl Write) -> Result<u16> {
        Self::write_chunk_header(0x2, 0, w)?;
        Ok(0)
    }

    /// Writes a payload chunk containing the given data to the given writer.
    fn write_payload(data: &[u8], w: &mut impl Write) -> Result<u16> {
        assert!(data.len() <= u16::MAX as usize);

        Self::write_chunk_header(0x0, data.len() as u16, w)?;
        w.write_all(&data)?;

        Ok(data.len() as u16)
    }

    /// Writes a SACK chunk containing the given data to the given writer.
    fn write_sack(
        ack_no: u32,
        nak_list: &[u32],
        w: &mut impl Write,
    ) -> Result<u16> {
        let payload_len = U32_SIZE as usize +
            nak_list.len() as usize * U32_SIZE as usize;

        assert!(payload_len <= u16::MAX as usize);
        Self::write_chunk_header(0x4, payload_len as u16, w)?;

        w.write_u32::<NetworkEndian>(ack_no)?;

        for &nak_no in nak_list {
            w.write_u32::<NetworkEndian>(nak_no)?;
        }

        Ok(payload_len as u16)
    }

    /// Writes the security flag chunk to the given writer.
    fn write_security_flag(is_insecure: bool, w: &mut impl Write) -> Result<u16> {
        Self::write_chunk_header(0xfe, 1, w)?;
        w.write_u8(if is_insecure { 1 } else { 0 })?;

        Ok(1)
    }

    /// Writes a SYN to the given writer.
    fn write_syn(w: &mut impl Write) -> Result<u16> {
        Self::write_chunk_header(0x1, 0, w)?;
        Ok(0)
    }

    /// Writes an unknown chunk of the given type.
    fn write_unknown(ty: u16, data: &[u8], w: &mut impl Write) -> Result<u16> {
        assert!(data.len() <= u16::MAX as usize);

        Self::write_chunk_header(ty, data.len() as u16, w)?;
        w.write_all(data)?;

        Ok(data.len() as u16)
    }

    /// Writes chunk type and payload length to the given writer.
    fn write_chunk_header(ty: u16, len: u16, w: &mut impl Write) -> Result<()> {
        w.write_u16::<NetworkEndian>(ty)?;
        w.write_u16::<NetworkEndian>(len)?;

        Ok(())
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

impl Segment {
    /// Reads a segment from the given reader.
    ///
    /// It is strongly advised to pass a buffering `io.Read` implementation
    /// since the parser will issue lots of small calls to `read`.
    pub fn read_from(r: &mut impl Read) -> Result<Segment> {
        let sq_no = r.read_u32::<NetworkEndian>()?;
        let window_size = r.read_u32::<NetworkEndian>()?;

        let mut chunk_list = Vec::new();
        while let Some(ch) = Chunk::read_from(r)? {
            chunk_list.push(ch);
        }

        Ok(Segment {
            chunks: chunk_list,
            seq_no: sq_no,
            window_size: window_size,
        })
    }

    /// Writes the segment to the given writer.
    ///
    /// It is strongly advised to pass a buffering `io.Write` implementation
    /// since the serializer will issue lots of small calls to `write`.
    pub fn write_to(&self, w: &mut impl Write) -> Result<()> {
        w.write_u32::<NetworkEndian>(self.seq_no)?;
        w.write_u32::<NetworkEndian>(self.window_size)?;

        for ch in &self.chunks {
            ch.write_to(w)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, ErrorKind};
    use super::*;

    /// Test deserialization of flag chunks.
    #[test]
    fn deserialize_flag_chunks() {
        fn deserialize_flag_chunk(ty: u8, should: Chunk) {
            // Check the base case
            let mut data = Cursor::new(vec![0x0, ty, 0x0, 0x0]);
            assert_eq!(
                Chunk::read_from(&mut data).unwrap().unwrap(),
                should,
            );

            // Now check if we correctly parse (technically) incorrect lengths.
            // The following should all parse equivalent, since we're padding
            // to multiples of 32 bits.
            let data = vec![
                Cursor::new(vec![0x0, ty, 0x0, 0x1, 0x0, 0x0, 0x0, 0x0]),
                Cursor::new(vec![0x0, ty, 0x0, 0x2, 0x0, 0x0, 0x0, 0x0]),
                Cursor::new(vec![0x0, ty, 0x0, 0x3, 0x0, 0x0, 0x0, 0x0]),
                Cursor::new(vec![0x0, ty, 0x0, 0x4, 0x0, 0x0, 0x0, 0x0]),
            ];
            for mut cur in data.into_iter() {
                assert_eq!(
                    Chunk::read_from(&mut cur).unwrap().unwrap(),
                    should,
                );
            }
        }

        deserialize_flag_chunk(0x1, Chunk::Syn);
        deserialize_flag_chunk(0x2, Chunk::Fin);
        deserialize_flag_chunk(0x3, Chunk::Abort);
    }

    #[test]
    fn deserialize_sack_chunk() {
        let mut data = Cursor::new(vec![
            0x0, 0x4, 0x0, 0xc,
            0x0, 0x0, 0x0, 0x4,
            0x0, 0x0, 0x0, 0x6,
            0x0, 0x0, 0x0, 0x5,
        ]);

        let chunk = Chunk::read_from(&mut data).unwrap().unwrap();
        match chunk {
            Chunk::Sack { ack_no, nak_list } => {
                assert_eq!(ack_no, 4);
                assert_eq!(nak_list.len(), 2);
                assert_eq!(nak_list[0], 6);
                assert_eq!(nak_list[1], 5);
            },
            x => panic!(format!("wrong chunk type {:?}", x)),
        }
    }

    #[test]
    #[should_panic]
    fn deserialize_sack_chunk_empty() {
        let mut data = Cursor::new(vec![
            0x0, 0x4, 0x0, 0x0,
        ]);

        Chunk::read_from(&mut data).unwrap().unwrap();
    }

    #[test]
    #[should_panic]
    fn deserialize_sack_chunk_invalid_length() {
        let mut data = Cursor::new(vec![
            0x0, 0x4, 0x0, 0x4,
        ]);

        Chunk::read_from(&mut data).unwrap().unwrap();
    }

    #[test]
    fn deserialize_sack_chunk_empty_nak_list() {
        let mut data = Cursor::new(vec![
            0x0, 0x4, 0x0, 0x4,
            0x0, 0x0, 0x0, 0x4,
        ]);

        let chunk = Chunk::read_from(&mut data).unwrap().unwrap();
        match chunk {
            Chunk::Sack { ack_no, nak_list } => {
                assert_eq!(ack_no, 4);
                assert_eq!(nak_list.len(), 0);
            },
            x => panic!(format!("wrong chunk type {:?}", x)),
        }
    }

    /// Test serialization of flag chunks.
    #[test]
    fn serialize_flag_chunks() {
        fn serialize_flag_chunk(ch: Chunk, expected_type: u8) {
            let mut buf = Cursor::new(vec![]);
            ch.write_to(&mut buf).unwrap();

            assert_eq!(
                &buf.into_inner(),
                &[0x0, expected_type, 0x0, 0x0],
            );
        }

        serialize_flag_chunk(Chunk::Syn, 0x1);
        serialize_flag_chunk(Chunk::Fin, 0x2);
        serialize_flag_chunk(Chunk::Abort, 0x3);
    }

    #[test]
    fn serialize_sack_chunk() {
        let chunk = Chunk::Sack {
            ack_no: 17,
            nak_list: vec![20, 18],
        };

        let mut buf = Vec::new();
        chunk.write_to(&mut buf).unwrap();

        let expected = &[
            0x0, 0x4, 0x0, 0xc,
            0x0, 0x0, 0x0, 0x11,
            0x0, 0x0, 0x0, 0x14,
            0x0, 0x0, 0x0, 0x12,
        ];
        assert_eq!(&buf, expected);
    }

    #[test]
    fn serialize_sack_chunk_empty_nak_list() {
        let chunk = Chunk::Sack {
            ack_no: 17,
            nak_list: Vec::new(),
        };

        let mut buf = Vec::new();
        chunk.write_to(&mut buf).unwrap();

        let expected = &[
            0x0, 0x4, 0x0, 0x4,
            0x0, 0x0, 0x0, 0x11,
        ];
        assert_eq!(&buf, expected);
    }

    /// Ensure we cannot read data from nothing.
    #[test]
    fn zero_input() {
        let mut data = Cursor::new(vec![]);

        match Segment::read_from(&mut data) {
            Ok(_) => panic!("Read segment from empty data."),
            Err(ref e) if e.kind() != ErrorKind::UnexpectedEof =>
                panic!("Unexpected error kind"),
            _ => {},
        }
        match Segment::read_from(&mut data) {
            Ok(_) => panic!("Read chunk from empty data."),
            Err(ref e) if e.kind() != ErrorKind::UnexpectedEof =>
                panic!("Unexpected error kind"),
            _ => {},
        }
    }
}
