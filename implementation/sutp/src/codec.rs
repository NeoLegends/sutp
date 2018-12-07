use bytes::{BufMut, BytesMut};
use std::io::{self, Cursor, Error, ErrorKind, Read};
use tokio::codec::{Decoder, Encoder};

use crate::segment::Segment;

/// An implementation of a (de)serializer of SUTP segments.
///
/// This mostly just wraps `Segment::read_from` and `Segment::write_to`.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub struct SutpCodec;

/// A wrapper around an io::Read that counts how many bytes were read.
#[derive(Debug)]
struct CountingReader<R> {
    count: usize,
    reader: R,
}

impl Decoder for SutpCodec {
    type Item = Segment;
    type Error = Error;

    fn decode(
        &mut self,
        src: &mut BytesMut,
    ) -> Result<Option<Self::Item>, Self::Error> {
        let (segment, count) = {
            // To properly advance the buffer we need to count how many bytes
            // we've read in the case that everything parses okay.

            let mut counter = CountingReader::new(Cursor::new(src.as_mut()));

            match Segment::read_from_with_crc32(&mut counter) {
                Ok(segment) => (segment, counter.count()),
                Err(ref err) if err.kind() == ErrorKind::UnexpectedEof =>
                    return Ok(None),
                Err(e) => return Err(e),
            }
        };

        src.advance(count);
        Ok(Some(segment))
    }
}

impl Encoder for SutpCodec {
    type Item = Segment;
    type Error = Error;

    fn encode(
        &mut self,
        item: Segment,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        item.write_to_with_crc32(&mut dst.writer())
    }
}

impl<R> CountingReader<R> {
    pub fn new(reader: R) -> Self {
        CountingReader {
            count: 0,
            reader: reader,
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.reader.read(buf)?;
        self.count += read;
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use super::*;

    #[test]
    fn counting_read() {
        let data = Cursor::new(vec![1, 2, 3, 4, 5]);
        let mut reader = CountingReader::new(data);

        let mut target = vec![0; 10];
        assert_eq!(reader.read(&mut target).unwrap(), 5);
        assert_eq!(reader.count(), 5);
    }
}
