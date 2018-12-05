//! This module implements the data format of segments as specified in
//! https://laboratory.comsys.rwth-aachen.de/sutp/data-format/blob/master/README.md.

use byteorder::{NetworkEndian, ReadBytesExt, WriteBytesExt};
use chunk::Chunk;
use flate2::{CrcReader, CrcWriter};
use std::io::{self, Error, ErrorKind, Read, Write};

/// An SUTP segment.
#[derive(Clone, Debug, Default, Hash, Eq, PartialEq)]
pub struct Segment {
    pub chunks: Vec<Chunk>,
    pub seq_no: u32,
    pub window_size: u32,
}

/// An error that can occur during segment validation.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum ValidationError {
    /// There is a conflict within the chunk's meanings
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

    /// Validates the segment's contents.
    pub fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();

        if self.chunks.is_empty() {
            errors.push(ValidationError::NoChunks);
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
            Err(errors)
        }
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

    /// Checks whether the segment contains chunk conflicts.
    fn check_conflicts(&self) -> Option<ValidationError> {
        let contains_abrt = self.chunks.iter()
            .any(|ch| ch.is_abrt());
        let all_abort = self.chunks.iter()
            .all(|ch| ch.is_abrt());

        if contains_abrt && !all_abort {
            Some(ValidationError::Conflict)
        } else {
            None
        }
    }

    /// Checks whether the segment contains illegal duplication and returns an
    /// appropriate error.
    ///
    /// For example, flag chunks cannot be duplicated to avoid ambiguities.
    fn check_illegal_duplication(&self) -> Option<ValidationError> {
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
            Some(ValidationError::DuplicatedChunks {
                abrt: abrt_count > 0,
                fin: fin_count > 0,
                syn: syn_count > 0,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, ErrorKind};
    use super::*;

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
