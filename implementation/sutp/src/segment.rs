//! This module implements the data format of segments as specified in
//! https://laboratory.comsys.rwth-aachen.de/sutp/data-format/blob/master/README.md.

use byteorder::{NetworkEndian, ReadBytesExt, WriteBytesExt};
use flate2::{CrcReader, CrcWriter};
use std::{
    error::Error as StdError,
    fmt::{Display, Formatter, Result as FmtResult},
    io::{self, Error, ErrorKind, Read, Write}
};

use crate::ResultExt;
use crate::chunk::Chunk;

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
    DuplicatedChunks {
        abrt: bool,
        fin: bool,
        syn: bool,
    },

    /// The segment does not contain any chunks.
    NoChunks,
}

/// The error when segment validation is not successful.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ValidationError {
    issues: Vec<Issue>,
}

impl Segment {
    /// Checks whether the segment can be classified as a "SYN->" or "SYN 1"
    /// segment. That is, the very first segment on the wire used to intiate
    /// a connection.
    pub fn is_syn1(&self) -> bool {
        let contains_syn = self.chunks.iter()
            .any(|ch| ch.is_syn());
        let contains_sack = self.chunks.iter()
            .any(|ch| ch.is_sack());

        contains_syn && !contains_sack
    }

    /// Checks whether the segment can be classified as a "<-SYN" or "SYN 2"
    /// segment. That is, the second segment sent in response to the first
    /// segment on the wire.
    pub fn is_syn2(&self, syn1_sq_no: u32) -> bool {
        let contains_syn = self.chunks.iter()
            .any(|ch| ch.is_syn());
        let contains_acking_sack = self.chunks.iter()
            .filter(|ch| ch.is_sack())
            .any(|sack| match sack {
                Chunk::Sack(ack_no, _) => *ack_no == syn1_sq_no,
                _ => unreachable!(),
            });

        contains_syn && contains_acking_sack
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
        let contains_abrt = self.chunks.iter()
            .any(|ch| ch.is_abrt());
        let all_abort = self.chunks.iter()
            .all(|ch| ch.is_abrt());

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
        let abrt_count = self.chunks.iter()
            .filter(|ch| ch.is_abrt())
            .count();
        let fin_count = self.chunks.iter()
            .filter(|ch| ch.is_fin())
            .count();
        let syn_count = self.chunks.iter()
            .filter(|ch| ch.is_syn())
            .count();

        if abrt_count > 0 || fin_count > 0 || syn_count > 0 {
            Some(Issue::DuplicatedChunks {
                abrt: abrt_count > 0,
                fin: fin_count > 0,
                syn: syn_count > 0,
            })
        } else {
            None
        }
    }
}

impl Segment {
    /// Reads a segment from the given reader.
    ///
    /// It is strongly advised to pass a buffering `io.Read` implementation
    /// since the parser will issue lots of small calls to `read`.
    pub fn read_from(r: &mut impl Read) -> io::Result<Segment> {
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

    /// Reads a segment from the given reader and verifies the CRC-32 signature.
    ///
    /// It is strongly advised to pass a buffering `io.Read` implementation
    /// since the parser will issue lots of small calls to `read`.
    pub fn read_from_with_crc32(r: &mut impl Read) -> io::Result<Segment> {
        let mut crc_reader = CrcReader::new(r);
        let segment = Self::read_from(&mut crc_reader)?;

        let data_crc_sum = crc_reader.crc().sum();
        let segment_crc_sum = crc_reader.into_inner().read_u32::<NetworkEndian>()?;

        if data_crc_sum != segment_crc_sum {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "crc32 sum mismatch: got {:X}, wanted {:X}",
                    segment_crc_sum,
                    data_crc_sum,
                ),
            ));
        }

        Ok(segment)
    }

    /// Reads a segment from the given reader, verifies the CRC-32 signature
    /// and validates the contents.
    ///
    /// A validation error is reported through an IO error with kind `InvalidData`.
    ///
    /// It is strongly advised to pass a buffering `io.Read` implementation
    /// since the parser will issue lots of small calls to `read`.
    pub fn read_from_and_validate(r: &mut impl Read) -> io::Result<Segment> {
        Self::read_from_with_crc32(r)
            .inspect_mut(|segment| {
                segment.validate()
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
        crc_writer.into_inner().write_u32::<NetworkEndian>(crc_sum)?;

        Ok(())
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
    pub fn seq_no(mut self, sq_no: u32) -> Self {
        self.seq_no = Some(sq_no);
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
            },
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
        let joined = self.issues.iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("; ");

        write!(fmt, "segment validation failed: {}", joined)
    }
}

impl StdError for ValidationError {}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, ErrorKind};
    use super::*;

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
    }
}
