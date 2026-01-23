//! Generic support for input from both slice and reader sources.
//!
//! This module helps xt dynamically choose the most functional and efficient translation strategy
//! for any given input source and format. Several factors have influenced its design:
//!
//! - To translate unbounded input streams, we must be able to provide a reader to the input
//!   format's deserializer.
//!
//! - To support format detection based on parser trials without sacrificing general reader
//!   support, we must be able to rewind a potentially non-seekable input.
//!
//! - However, the ability to rewind a reader shouldn't impose extra costs when format detection is
//!   unnecessary.
//!
//! - Even for input formats that support readers, it's almost always faster to translate from an
//!   in-memory slice of the full input when one is readily available.
//!
//! - Not all input formats can translate from a reader, and must consume input from a slice no
//!   matter what.
//!
//! The module accounts for these factors by supporting both slice and reader inputs, exposing both
//! possibilities to formats that can optimize for each one, and providing slice-only convenience
//! methods for those that can't.
//!
//! For slice inputs, the module simply passes a borrowed slice directly to the format.
//!
//! For reader inputs, borrowing the input for format detection yields a [`CaptureReader`] that
//! transparently snoops on and replays the reader's earliest bytes as many times as necessary. If
//! a format happens to consume all of a borrowed reader, all future use of the input becomes
//! slice-based. Or, if format detection is skipped, taking ownership of the input yields the
//! original reader with no wrapping beyond boxing as a trait object.

use std::borrow::Cow;
use std::io::{self, Cursor, Read, Write};

/// A reusable container for xt's input.
///
/// See [the module documentation](self) and [`Translator`](crate::Translator) for details.
pub(crate) struct Handle<'i>(Source<'i>);

/// The private container for xt's original input source.
enum Source<'i> {
	Slice(&'i [u8]),
	Reader(GuardedCaptureReader<Box<dyn Read + 'i>>),
}

impl<'i> Handle<'i> {
	/// Creates a handle for an input slice.
	pub(crate) fn from_slice(b: &'i [u8]) -> Handle<'i> {
		Handle(Source::Slice(b))
	}

	/// Creates a handle for an input reader.
	pub(crate) fn from_reader<R>(r: R) -> Handle<'i>
	where
		R: Read + 'i,
	{
		Handle(Source::Reader(GuardedCaptureReader::new(Box::new(r))))
	}

	/// Borrows a temporary reference to the input.
	///
	/// For reader inputs, this may provide a [`CaptureReader`] or a slice depending on whether the
	/// reader's input is fully consumed. See [the module documentation](self) for details.
	pub(crate) fn borrow_mut(&mut self) -> Ref<'i, '_> {
		match &mut self.0 {
			Source::Slice(b) => Ref::Slice(b),
			Source::Reader(r) => {
				let r = r.rewind_and_borrow_mut();
				if r.is_source_eof() {
					Ref::Slice(r.captured())
				} else {
					Ref::Reader(r)
				}
			}
		}
	}
}

/// Produces the original input as a slice, either by passing through the original slice or
/// consuming the original reader.
impl<'i> TryFrom<Handle<'i>> for Cow<'i, [u8]> {
	type Error = io::Error;

	fn try_from(handle: Handle<'i>) -> io::Result<Cow<'i, [u8]>> {
		match handle.0 {
			Source::Slice(b) => Ok(Cow::Borrowed(b)),
			Source::Reader(r) => {
				let mut r = r.rewind_and_take();
				r.capture_to_end()?;
				let (cursor, _) = r.into_inner();
				Ok(Cow::Owned(cursor.into_inner()))
			}
		}
	}
}

/// A non-reusable container for xt's input, to be consumed by the selected input format.
///
/// For reader inputs, this may provide a reader or a slice depending on whether the reader's input
/// was fully consumed. See [the module documentation](self) for details.
pub(crate) enum Input<'i> {
	Slice(Cow<'i, [u8]>),
	Reader(Box<dyn Read + 'i>),
}

impl<'i> From<Handle<'i>> for Input<'i> {
	fn from(handle: Handle<'i>) -> Self {
		match handle.0 {
			Source::Slice(b) => Input::Slice(Cow::Borrowed(b)),
			Source::Reader(r) => {
				let r = r.rewind_and_take();
				let source_eof = r.is_source_eof();
				let (cursor, source) = r.into_inner();
				if source_eof {
					Input::Slice(Cow::Owned(cursor.into_inner()))
				} else if cursor.get_ref().is_empty() {
					Input::Reader(source)
				} else {
					Input::Reader(Box::new(FusedReader::new(cursor).chain(source)))
				}
			}
		}
	}
}

/// A temporary reference to xt's input created by [`Handle::borrow_mut`].
pub(crate) enum Ref<'i, 'h>
where
	'i: 'h,
{
	Slice(&'h [u8]),
	Reader(&'h mut CaptureReader<Box<dyn Read + 'i>>),
}

impl<'i, 'h> Ref<'i, 'h>
where
	'i: 'h,
{
	/// Returns a prefix of the input.
	///
	/// For reader inputs not fully consumed, `size_hint` represents the minimum size of the prefix
	/// that the call should try to produce by capturing new bytes from the source if necessary.
	/// The returned prefix may be smaller or larger than `size_hint` if the reader reaches EOF or
	/// more input is already captured.
	///
	/// For slice inputs and fully consumed reader inputs, this returns the full input regardless
	/// of `size_hint`.
	pub(crate) fn prefix(&mut self, size_hint: usize) -> io::Result<&[u8]> {
		match self {
			Ref::Slice(b) => Ok(b),
			Ref::Reader(r) => {
				r.capture_up_to_size(size_hint)?;
				Ok(r.captured())
			}
		}
	}
}

/// A wrapper that drops a reader as soon as it first reaches EOF.
///
/// As the first half of a [`Chain`](std::io::Chain), this cleans up the first reader's resources
/// as soon as the chain moves to the second reader, rather than when the whole `Chain` is dropped.
struct FusedReader<R>(Option<R>)
where
	R: Read;

impl<R> FusedReader<R>
where
	R: Read,
{
	fn new(r: R) -> FusedReader<R> {
		FusedReader(Some(r))
	}
}

impl<R> Read for FusedReader<R>
where
	R: Read,
{
	fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
		let n = match &mut self.0 {
			None => return Ok(0),
			Some(r) => r.read(buf)?,
		};
		if n == 0 && !buf.is_empty() {
			self.0 = None;
		}
		Ok(n)
	}
}

/// A wrapper that forces a [`CaptureReader`] to be rewound prior to use, which eliminates a class
/// of bugs xt has had in the past.
struct GuardedCaptureReader<R>(CaptureReader<R>)
where
	R: Read;

impl<R> GuardedCaptureReader<R>
where
	R: Read,
{
	fn new(r: R) -> Self {
		Self(CaptureReader::new(r))
	}

	fn rewind_and_borrow_mut(&mut self) -> &mut CaptureReader<R> {
		self.0.rewind();
		&mut self.0
	}

	fn rewind_and_take(mut self) -> CaptureReader<R> {
		self.0.rewind();
		self.0
	}
}

/// Captures and replays the output of an arbitrary non-seekable reader.
///
/// After calling [`rewind`](CaptureReader::rewind), a `CaptureReader` produces its captured bytes
/// before consuming more of the source, analogous to [`Seek::rewind`](std::io::Seek::rewind).
///
/// A `CaptureReader` also tracks end-of-file conditions from the source, so consumers can switch
/// to fully buffered input, analogous to [`Read::read_to_end`].
pub(crate) struct CaptureReader<R>
where
	R: Read,
{
	prefix: Cursor<Vec<u8>>,
	source: R,
	source_eof: bool,
}

impl<R> CaptureReader<R>
where
	R: Read,
{
	/// Creates a new reader that captures `source`.
	fn new(source: R) -> Self {
		Self {
			prefix: Cursor::new(vec![]),
			source,
			source_eof: false,
		}
	}

	/// Returns a slice of all captured input, starting from the beginning.
	fn captured(&self) -> &[u8] {
		self.prefix.get_ref()
	}

	/// Returns the number of bytes remaining to read from the captured prefix before consuming
	/// more from the source.
	fn captured_unread_size(&self) -> usize {
		// The cursor position is relative to an in-memory slice.
		// This shouldn't truncate unless we manually give the cursor
		// a ridiculous position.
		#[allow(clippy::cast_possible_truncation)]
		let offset = self.prefix.position() as usize;
		self.prefix.get_ref().len() - offset
	}

	/// Rewinds the reader, so that subsequent reads produce captured bytes before reading more
	/// from the source.
	fn rewind(&mut self) {
		self.prefix.set_position(0);
	}

	/// Captures all of the source's remaining input without modifying the reader's position.
	fn capture_to_end(&mut self) -> io::Result<()> {
		if !self.source_eof {
			self.source.read_to_end(self.prefix.get_mut())?;
			self.source_eof = true;
		}
		Ok(())
	}

	/// Attempts to read enough data from the source for the capture buffer to contain at least
	/// `size` bytes, without modifying the reader's position.
	///
	/// The actual number of captured bytes may be less than `size` if the source reaches EOF, or
	/// more than `size` if more of the source is already captured.
	fn capture_up_to_size(&mut self, size: usize) -> io::Result<()> {
		let needed = size.saturating_sub(self.prefix.get_ref().len());
		if needed == 0 {
			return Ok(());
		}

		let mut take = self.source.by_ref().take(needed as u64);
		take.read_to_end(self.prefix.get_mut())?;
		if take.limit() > 0 {
			self.source_eof = true;
		}
		Ok(())
	}

	/// Returns true if the latest read from the source indicated an EOF.
	fn is_source_eof(&self) -> bool {
		self.source_eof
	}

	/// Returns any captured prefix along with the source reader.
	fn into_inner(self) -> (Cursor<Vec<u8>>, R) {
		(self.prefix, self.source)
	}
}

impl<R> Read for CaptureReader<R>
where
	R: Read,
{
	fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
		// First, copy as much data as we can from the unread portion of the cursor into the
		// buffer.
		let prefix_size = std::cmp::min(buf.len(), self.captured_unread_size());
		self.prefix.read_exact(&mut buf[..prefix_size])?;
		if self.captured_unread_size() > 0 || prefix_size == buf.len() {
			return Ok(prefix_size);
		}

		// Second, fill the rest of the buffer with data from the source, and capture it for
		// ourselves too.
		//
		// The `read` docs recommend against us reading from `buf`, but also require callers to
		// assume we might do this. Letting our consumer drive the frequency and size of source
		// reads makes our presence more transparent to both sides. We try to be good citizens by
		// only reading the parts of `buf` the source tells us were freshly written.
		let buf = &mut buf[prefix_size..];
		let source_size = self.source.read(buf)?;
		self.prefix.write_all(&buf[..source_size])?;

		// Finally, mark whether the source is at EOF (keeping in mind that it can technically
		// return more data after an EOF). We know `buf` can't be empty since we return early when
		// `prefix_size == buf.len()`, so a
		// 0 byte read must mean EOF.
		self.source_eof = source_size == 0;

		Ok(prefix_size + source_size)
	}
}

#[cfg(test)]
mod tests {
	use super::{CaptureReader, Handle, Input, Ref};
	use std::borrow::Cow;
	use std::io::{self, Cursor, Read};

	const DATA: &str = "abcdefghij";
	const HALF: usize = DATA.len() / 2;

	#[test]
	fn input_borrow_mut_rewind() {
		let mut handle = Handle::from_reader(DATA.as_bytes());
		let mut buf = vec![];

		let mut input_ref = handle.borrow_mut();
		match input_ref {
			Ref::Slice(_) => unreachable!(),
			Ref::Reader(ref mut r) => r.take(HALF as u64).read_to_end(&mut buf).unwrap(),
		};
		assert_eq!(std::str::from_utf8(&buf), Ok(&DATA[..HALF]));
		buf.clear();

		// `Ref`s are designed to be forgettable without breaking behavior. The intent of this test
		// is to ensure that no future `Drop` impl breaks this expectation.
		#[allow(clippy::forget_non_drop)]
		std::mem::forget(input_ref);

		match handle.borrow_mut() {
			Ref::Slice(_) => unreachable!(),
			Ref::Reader(r) => r.take(HALF as u64).read_to_end(&mut buf).unwrap(),
		};
		assert_eq!(std::str::from_utf8(&buf), Ok(&DATA[..HALF]));
		buf.clear();

		// If we only consume part of a borrowed reader, we need to reset the reader before giving
		// ownership away.
		let mut r = match handle.into() {
			Input::Slice(_) => unreachable!(),
			Input::Reader(r) => r,
		};
		assert!(matches!(r.read_to_end(&mut buf), Ok(len) if len == DATA.len()));
		assert_eq!(std::str::from_utf8(&buf), Ok(DATA));
	}

	#[test]
	fn input_into_cow() {
		let mut handle = Handle::from_reader(DATA.as_bytes());

		match handle.borrow_mut() {
			Ref::Slice(_) => unreachable!(),
			Ref::Reader(r) => io::copy(&mut r.take(HALF as u64), &mut io::sink()).unwrap(),
		};

		// If we only consume part of a borrowed reader, turning the input into a slice should
		// still produce the full input.
		let buf: Cow<'_, [u8]> = handle.try_into().unwrap();
		assert_eq!(std::str::from_utf8(&buf), Ok(DATA));
	}

	#[test]
	fn capture_reader_straight_read() {
		let mut r = CaptureReader::new(Cursor::new(String::from(DATA)));

		assert_eq!(io::read_to_string(&mut r).unwrap(), DATA);
		assert!(r.is_source_eof());

		let (cursor, _) = r.into_inner();
		assert!(matches!(std::str::from_utf8(cursor.get_ref()), Ok(DATA)));
	}

	#[test]
	fn capture_reader_rewind() {
		let mut r = CaptureReader::new(Cursor::new(String::from(DATA)));

		let mut tmp = [0; HALF];
		assert!(matches!(r.read_exact(&mut tmp), Ok(())));
		assert_eq!(std::str::from_utf8(&tmp), Ok(&DATA[..HALF]));
		assert_eq!(std::str::from_utf8(r.captured()), Ok(&DATA[..HALF]));
		assert!(!r.is_source_eof());

		r.rewind();

		assert_eq!(io::read_to_string(&mut r).unwrap(), DATA);
		assert_eq!(r.captured(), DATA.as_bytes());
		assert!(r.is_source_eof());
	}

	#[test]
	fn capture_reader_to_end() {
		let mut r = CaptureReader::new(Cursor::new(String::from(DATA)));
		assert!(r.capture_to_end().is_ok());
		assert_eq!(std::str::from_utf8(r.captured()), Ok(DATA));
		assert!(r.is_source_eof());
	}

	#[test]
	fn capture_reader_up_to() {
		let mut r = CaptureReader::new(Cursor::new(String::from(DATA)));
		assert!(r.capture_up_to_size(HALF).is_ok());
		assert_eq!(std::str::from_utf8(r.captured()), Ok(&DATA[..HALF]));
		assert!(!r.is_source_eof());
	}
}
