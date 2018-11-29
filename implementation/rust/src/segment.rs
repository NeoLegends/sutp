use byteorder::{NetworkEndian, ReadBytesExt, WriteBytesExt};
use chunk::Chunk;
use std::io::{Read, Result, Write};

/// An SUTP segment.
#[derive(Clone, Debug, Default, Hash, Eq, PartialEq)]
pub struct Segment {
    pub chunks: Vec<Chunk>,
    pub seq_no: u32,
    pub window_size: u32,
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
