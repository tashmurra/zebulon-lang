//! Bounded host-result bytes, separate from game values and their ownership forest.
use crate::objects::Error;

#[derive(Default)]
pub(crate) struct Output {
    bytes: Vec<u8>,
    limit: usize,
}
impl Output {
    pub(crate) fn reset(&mut self, limit: usize) {
        self.bytes = Vec::new();
        self.limit = limit;
    }
    pub(crate) fn append(&mut self, text: &str) -> Result<(), Error> {
        let length = self
            .bytes
            .len()
            .checked_add(text.len())
            .ok_or(Error::ResourceLimit)?;
        if length > self.limit {
            return Err(Error::ResourceLimit);
        }
        self.bytes
            .try_reserve(text.len())
            .map_err(|_| Error::Allocation)?;
        self.bytes.extend_from_slice(text.as_bytes());
        Ok(())
    }
    /// Take the bytes gathered so far and start again. The runtime does not
    /// write them anywhere: they are handed to the host, which
    /// owns the console. The byte limit therefore applies per handover rather
    /// than per session.
    pub(crate) fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.bytes)
    }

    /// The bytes gathered since the last handover, left in place.
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn byte(&self, offset: u64) -> u32 {
        usize::try_from(offset)
            .ok()
            .and_then(|i| self.bytes.get(i))
            .map_or(256, |b| u32::from(*b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_admission_is_atomic_and_reset_releases_previous_result() {
        let mut out = Output::default();
        out.reset(4);
        out.append("a\0").unwrap();
        out.append("é").unwrap();
        assert_eq!(out.append("!"), Err(Error::ResourceLimit));
        assert_eq!(
            (
                out.byte(0),
                out.byte(1),
                out.byte(2),
                out.byte(3),
                out.byte(4)
            ),
            (97, 0, 195, 169, 256)
        );
        out.reset(0);
        assert_eq!(out.byte(0), 256);
        assert!(out.append("").is_ok());
    }

    #[test]
    fn handing_the_bytes_over_clears_the_buffer_and_restarts_the_limit() {
        let mut out = Output::default();
        out.reset(2);
        out.append("ab").unwrap();
        assert_eq!(out.append("c"), Err(Error::ResourceLimit));
        assert_eq!(out.take(), b"ab");
        assert_eq!(out.byte(0), 256);
        // The same limit now applies to the bytes gathered since the handover.
        out.append("cd").unwrap();
        assert_eq!(out.append("e"), Err(Error::ResourceLimit));
        assert_eq!((out.byte(0), out.byte(1)), (99, 100));
        assert_eq!(out.take(), b"cd");
        assert!(out.take().is_empty());
    }
}
