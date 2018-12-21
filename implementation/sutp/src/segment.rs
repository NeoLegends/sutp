//! This module implements the data format of segments as specified in
//! https://laboratory.comsys.rwth-aachen.de/sutp/data-format/blob/master/README.md.

use crate::{
    chunk::{Chunk, CompressionAlgorithm},
    ResultExt,
};
use byteorder::{ByteOrder, NetworkEndian, WriteBytesExt};
use bytes::{Buf, Bytes, IntoBuf};
use flate2::{Crc, CrcWriter};
use std::{
    error::Error as StdError,
    fmt::{Display, Formatter, Result as FmtResult},
    io::{self, Cursor, Error, ErrorKind, Write},
    mem,
};

/// The overhead in bytes of serializing a single segment, besides the
/// size of the chunks.
const BINARY_OVERHEAD: usize = 12;

/// Whether a segment is ACKed or NAKed or whether the status is unknown.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub struct AckNak(Option<bool>);

/// An SUTP segment.
#[derive(Clone, Debug, Default, Hash, Eq, PartialEq)]
pub struct Segment {
    pub chunks: Vec<Chunk>,
    pub seq_no: u32,
    pub window_size: u32,
}

/// An builder for an SUTP segment.
#[derive(Clone, Debug, Default, Hash, Eq, PartialEq)]
pub struct SegmentBuilder {
    chunks: Vec<Chunk>,
    seq_no: Option<u32>,
    window_size: Option<u32>,
}

/// A specific segment validation issue.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum Issue {
    /// There is a conflict within the chunk's meanings.
    ///
    /// E. g. a SYN and an ABRT together in the same chunk don't make sense.
    Conflict,

    /// There is illegal chunk duplication.
    DuplicatedChunks { abrt: bool, fin: bool, syn: bool },

    /// The segment does not contain any chunks.
    NoChunks,
}

/// The error when segment validation is not successful.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ValidationError {
    issues: Vec<Issue>,
}

impl AckNak {
    /// Checks whether the AckNak is the ACK variant.
    pub fn is_ack(self) -> bool {
        match self {
            AckNak(Some(v)) => v,
            _ => false,
        }
    }

    /// Whether the ACK / NAK state is known.
    pub fn is_known(self) -> bool {
        match self {
            AckNak(Some(_)) => true,
            _ => false,
        }
    }

    /// Checks whether the AckNak is the NAK variant.
    pub fn is_nak(self) -> bool {
        match self {
            AckNak(Some(v)) => !v,
            _ => false,
        }
    }
}

impl Segment {
    /// Checks whether this segment ACKs or NAKs the given one.
    pub fn ack(&self, other_seq_no: u32) -> AckNak {
        self.chunks
            .iter()
            .filter_map(|ch| match ch {
                Chunk::Sack(ack, nak_list) => Some((ack, nak_list)),
                _ => None,
            })
            .filter_map(|(&ack, nak_list)| {
                let is_nakd = nak_list.iter().any(|&nak| other_seq_no == nak);

                if is_nakd {
                    Some(AckNak(Some(false)))
                } else if ack >= other_seq_no {
                    Some(AckNak(Some(true)))
                } else {
                    None
                }
            })
            .next()
            .unwrap_or(AckNak(None))
    }

    /// Gets the length of the segment in bytes if it were serialized in
    /// serialized form.
    pub fn binary_len(&self) -> usize {
        let chunk_len: usize = self.chunks.iter().map(|ch| ch.binary_len()).sum();

        chunk_len + BINARY_OVERHEAD // 64 bit header + 32 bit CRC
    }

    /// Checks whether the segment can be classified as a "SYN->" or "SYN 1"
    /// segment. That is, the very first segment on the wire used to intiate
    /// a connection.
    pub fn is_syn1(&self) -> bool {
        let contains_syn = self.chunks.iter().any(|ch| ch.is_syn());
        let contains_sack = self.chunks.iter().any(|ch| ch.is_sack());

        contains_syn && !contains_sack
    }

    /// Checks whether the segment can be classified as a "<-SYN" or "SYN 2"
    /// segment. That is, the second segment sent in response to the first
    /// segment on the wire.
    pub fn is_syn2_and_acks(&self, syn1_seq_no: u32) -> bool {
        let contains_syn = self.chunks.iter().any(|ch| ch.is_syn());

        contains_syn && self.ack(syn1_seq_no).is_ack()
    }

    /// Selects the most-preferred supported compression algorithm,
    /// if one is present.
    pub fn select_compression_alg(&self) -> Option<CompressionAlgorithm> {
        self.chunks
            .iter()
            .filter(|ch| ch.is_compression_negotiation())
            .flat_map(|ch| match ch {
                Chunk::CompressionNegotiation(list) => list,
                _ => unreachable!(),
            })
            .find(|alg| alg.is_known())
            .cloned()
    }

    /// Validates the segment's contents for the general case.
    ///
    /// This checks things like conflicting flag chunks, etc.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let mut errors = Vec::new();

        if self.chunks.is_empty() {
            errors.push(Issue::NoChunks);
        }
        if let Some(e) = self.check_illegal_duplication() {
            errors.push(e);
        }
        if let Some(e) = self.check_conflicts() {
            errors.push(e);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError::new(errors))
        }
    }

    /// Checks whether the segment contains chunk conflicts.
    fn check_conflicts(&self) -> Option<Issue> {
        let contains_abrt = self.chunks.iter().any(|ch| ch.is_abrt());
        let all_abort = self.chunks.iter().all(|ch| ch.is_abrt());

        if contains_abrt && !all_abort {
            Some(Issue::Conflict)
        } else {
            None
        }
    }

    /// Checks whether the segment contains illegal duplication and returns an
    /// appropriate error.
    ///
    /// For example, flag chunks cannot be duplicated to avoid ambiguities.
    fn check_illegal_duplication(&self) -> Option<Issue> {
        let abrt_count = self.chunks.iter().filter(|ch| ch.is_abrt()).count();
        let fin_count = self.chunks.iter().filter(|ch| ch.is_fin()).count();
        let syn_count = self.chunks.iter().filter(|ch| ch.is_syn()).count();

        if abrt_count > 1 || fin_count > 1 || syn_count > 1 {
            Some(Issue::DuplicatedChunks {
                abrt: abrt_count > 1,
                fin: fin_count > 1,
                syn: syn_count > 1,
            })
        } else {
            None
        }
    }
}

impl Segment {
    /// Reads a segment from the given buffer.
    pub fn read_from(r: &mut Bytes) -> io::Result<Segment> {
        let mut buf = r.as_ref().into_buf();
        assert_size!(buf, mem::size_of::<u32>() * 2);

        let seq_no = buf.get_u32_be();
        let window_size = buf.get_u32_be();

        r.advance(mem::size_of::<u32>() * 2);

        let mut chunk_list = Vec::new();
        while let Some(ch) = Chunk::read_from(r)? {
            chunk_list.push(ch);
        }

        Ok(Segment {
            chunks: chunk_list,
            seq_no,
            window_size,
        })
    }

    /// Reads a segment from the given buffer and verifies the CRC-32 signature.
    pub fn read_from_with_crc32(r: &mut Bytes) -> io::Result<Segment> {
        const U32_SIZE: usize = mem::size_of::<u32>();

        if r.len() < U32_SIZE {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }

        let (data_part, crc_part) = {
            let slice = &r;
            slice.split_at(slice.len() - U32_SIZE)
        };
        let actual_crc = {
            let mut crc = Crc::new();
            crc.update(data_part);
            crc.sum()
        };
        let expected_crc = NetworkEndian::read_u32(crc_part);

        if actual_crc != expected_crc {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "crc32 sum mismatch: got {:X}, wanted {:X}",
                    actual_crc, expected_crc,
                ),
            ));
        }

        let segment = Self::read_from(r)?;
        Ok(segment)
    }

    /// Reads a segment from the given reader, verifies the CRC-32 signature
    /// and validates the contents.
    ///
    /// A validation error is reported through an IO error with kind `InvalidData`.
    ///
    /// It is strongly advised to pass a buffering `io.Read` implementation
    /// since the parser will issue lots of small calls to `read`.
    pub fn read_from_and_validate(r: &mut Bytes) -> io::Result<Segment> {
        Self::read_from_with_crc32(r).inspect_mut(|segment| {
            segment
                .validate()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        })
    }

    /// Writes the segment to the given writer.
    ///
    /// It is strongly advised to pass a buffering `io.Write` implementation
    /// since the serializer will issue lots of small calls to `write`.
    pub fn write_to(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_u32::<NetworkEndian>(self.seq_no)?;
        w.write_u32::<NetworkEndian>(self.window_size)?;

        for ch in &self.chunks {
            ch.write_to(w)?;
        }

        Ok(())
    }

    /// Writes the segment including its CRC-32-sum to the given writer.
    ///
    /// It is strongly advised to pass a buffering `io.Write` implementation
    /// since the serializer will issue lots of small calls to `write`.
    pub fn write_to_with_crc32(&self, w: &mut impl Write) -> io::Result<()> {
        let mut crc_writer = CrcWriter::new(w);
        self.write_to(&mut crc_writer)?;

        let crc_sum = crc_writer.crc().sum();
        crc_writer
            .into_inner()
            .write_u32::<NetworkEndian>(crc_sum)?;

        Ok(())
    }

    /// Writes the segment to a new vector of bytes.
    pub fn to_vec(&self) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        self.write_to_with_crc32(&mut buf).unwrap();
        buf.into_inner()
    }
}

impl From<Segment> for Vec<u8> {
    fn from(sgmt: Segment) -> Self {
        sgmt.to_vec()
    }
}

impl SegmentBuilder {
    /// Constructs a new segment builder.
    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            seq_no: None,
            window_size: None,
        }
    }

    /// Builds the final segment.
    ///
    /// # Panics
    ///
    /// Panics if the sequence number or the window size wasn't set.
    pub fn build(self) -> Segment {
        Segment {
            chunks: self.chunks,
            seq_no: self.seq_no.expect("missing sequence number"),
            window_size: self.window_size.expect("missing window size"),
        }
    }

    /// Sets the sequence number.
    pub fn seq_no(mut self, seq_no: u32) -> Self {
        self.seq_no = Some(seq_no);
        self
    }

    /// Sets the window size.
    pub fn window_size(mut self, size: u32) -> Self {
        self.window_size = Some(size);
        self
    }

    /// Adds a chunk to the list of chunks.
    pub fn with_chunk(mut self, ch: Chunk) -> Self {
        self.chunks.push(ch);
        self
    }
}

impl Display for Issue {
    fn fmt(&self, fmt: &mut Formatter) -> FmtResult {
        match *self {
            Issue::Conflict => "conflicting chunks".fmt(fmt),
            Issue::DuplicatedChunks { abrt, fin, syn } => {
                let mut duplicated = Vec::with_capacity(3);
                if abrt {
                    duplicated.push("ABRT");
                }
                if fin {
                    duplicated.push("FIN");
                }
                if syn {
                    duplicated.push("SYN");
                }

                let joined = duplicated.join(", ");
                write!(fmt, "duplicated {}", joined)
            }
            Issue::NoChunks => "empty chunk list".fmt(fmt),
        }
    }
}

impl ValidationError {
    /// Constructs a new validation error from the given, non-empty
    /// list of issues.
    ///
    /// # Panics
    ///
    /// Panics if the issues list is empty.
    pub fn new(issues: Vec<Issue>) -> Self {
        assert!(!issues.is_empty());

        Self { issues }
    }

    /// Obtains the underlying issues.
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }
}

impl Display for ValidationError {
    fn fmt(&self, fmt: &mut Formatter) -> FmtResult {
        let joined = self
            .issues
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("; ");

        write!(fmt, "segment validation failed: {}", joined)
    }
}

impl StdError for ValidationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    #[test]
    fn binary_len() {
        let sq_empty = SegmentBuilder::new().seq_no(20).window_size(10).build();
        assert_eq!(sq_empty.binary_len(), BINARY_OVERHEAD);

        let sq_1_ch = SegmentBuilder::new()
            .seq_no(20)
            .window_size(10)
            .with_chunk(Chunk::Syn)
            .build();

        assert_eq!(sq_1_ch.binary_len(), BINARY_OVERHEAD + 4);
    }

    #[test]
    fn builder() {
        let sq = SegmentBuilder::new()
            .seq_no(20)
            .window_size(10)
            .with_chunk(Chunk::Syn)
            .build();

        let expected = Segment {
            chunks: vec![Chunk::Syn],
            seq_no: 20,
            window_size: 10,
        };

        assert_eq!(sq, expected);
    }

    #[test]
    fn select_compression_negotiation() {
        let segment = SegmentBuilder::new()
            .seq_no(1)
            .window_size(1)
            .with_chunk(Chunk::CompressionNegotiation(vec![
                CompressionAlgorithm::Gzip,
                CompressionAlgorithm::Snappy,
            ]))
            .build();

        assert_eq!(
            segment.select_compression_alg(),
            Some(CompressionAlgorithm::Gzip),
        );
    }

    #[test]
    fn select_compression_negotiation_empty() {
        let segment = SegmentBuilder::new()
            .seq_no(1)
            .window_size(1)
            .with_chunk(Chunk::CompressionNegotiation(Vec::new()))
            .build();

        assert!(segment.select_compression_alg().is_none());
    }

    #[test]
    fn select_compression_negotiation_unknown() {
        let segment = SegmentBuilder::new()
            .seq_no(1)
            .window_size(1)
            .with_chunk(Chunk::CompressionNegotiation(vec![
                CompressionAlgorithm::Unknown(100),
            ]))
            .build();

        assert!(segment.select_compression_alg().is_none());
    }

    /// Ensure we cannot read data from nothing.
    #[test]
    fn zero_input() {
        let mut data = Bytes::new();

        match Segment::read_from(&mut data) {
            Ok(_) => panic!("Read segment from empty data."),
            Err(ref e) if e.kind() != ErrorKind::UnexpectedEof => {
                panic!("Unexpected error kind")
            }
            _ => {}
        }
    }

    #[test]
    fn illegal_chunk_duplication_test() {
        let segment = SegmentBuilder::new()
            .seq_no(1)
            .window_size(1)
            .with_chunk(Chunk::Syn)
            .with_chunk(Chunk::Syn)
            .with_chunk(Chunk::Abort)
            .with_chunk(Chunk::Abort)
            .with_chunk(Chunk::Fin)
            .with_chunk(Chunk::Fin)
            .build();

        assert_eq!(
            Some(Issue::DuplicatedChunks {
                abrt: true,
                fin: true,
                syn: true,
            }),
            segment.check_illegal_duplication()
        );
    }

    #[test]
    fn no_illegal_chunk_duplication_test() {
        let segment = SegmentBuilder::new()
            .seq_no(1)
            .window_size(1)
            .with_chunk(Chunk::Syn)
            .with_chunk(Chunk::Abort)
            .with_chunk(Chunk::Fin)
            .build();

        assert_eq!(None, segment.check_illegal_duplication());
    }

    #[test]
    fn segment_and_chunk_parsing_test() {
        let segment = SegmentBuilder::new()
            .seq_no(1)
            .window_size(2)
            .with_chunk(Chunk::Syn)
            .with_chunk(Chunk::Abort)
            .with_chunk(Chunk::Sack(4, vec![5, 6, 7]))
            .with_chunk(Chunk::CompressionNegotiation(vec![
                CompressionAlgorithm::Gzip,
            ]))
            .with_chunk(Chunk::Payload(Bytes::new()))
            .with_chunk(Chunk::SecurityFlag(false))
            .with_chunk(Chunk::Fin)
            .build();
        let mut serialized_segment = segment.to_vec().into();

        assert_eq!(
            segment,
            Segment::read_from_with_crc32(&mut serialized_segment).unwrap(),
        );
    }
}
