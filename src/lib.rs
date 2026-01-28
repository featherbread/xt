//! Translations between serialized data formats.
//!
//! **xt is pre-1.0 software with an unstable library API!**
//!
//! To convert between serialized data formats in Rust code, consider the mature, stable,
//! and widely-used [`serde_transcode`][serde-transcode] crate instead.
//!
//! [serde-transcode]: https://docs.rs/serde-transcode

#![deny(
	// Enforce some additional strictness on unsafe code.
	unsafe_op_in_unsafe_fn,
	clippy::undocumented_unsafe_blocks,
	// Deny a number of `as` casts in favor of safer alternatives.
	clippy::as_underscore,
	clippy::ptr_as_ptr,
	clippy::cast_lossless,
	clippy::cast_possible_truncation,
	clippy::checked_conversions,
	clippy::unnecessary_cast,
	// More general style-type things.
	clippy::from_over_into,
	clippy::needless_raw_string_hashes,
	clippy::semicolon_if_nothing_returned,
)]
#![warn(
	// Print macros can panic, and should only be for temporary debugging.
	clippy::print_stderr,
	clippy::print_stdout,
	// The following macros represent incomplete implementation work.
	clippy::todo,
	clippy::unimplemented,
	// Style-type things that might not need an _immediate_ fix.
	clippy::doc_markdown,
	clippy::similar_names,
)]

use std::fmt;
use std::io::{self, Read, Write};

use serde::{de, ser};

mod error;
mod input;
mod json;
mod msgpack;
mod toml;
mod transcode;
mod yaml;

pub use error::{Error, Result};

/// Translates the contents of a single input slice to a different format.
///
/// See [`Translator::translate_slice`].
pub fn translate_slice<W>(input: &[u8], from: Option<Format>, to: Format, output: W) -> Result<()>
where
	W: Write,
{
	Translator::new(output, to).translate_slice(input, from)
}

/// Translates the contents of a single reader to a different format.
///
/// See [`Translator::translate_reader`].
pub fn translate_reader<R, W>(input: R, from: Option<Format>, to: Format, output: W) -> Result<()>
where
	R: Read,
	W: Write,
{
	Translator::new(output, to).translate_reader(input, from)
}

/// Translates multiple inputs to a single serialized output.
///
/// A `Translator` accepts both slice and reader input. See [`translate_slice`] and
/// [`translate_reader`] for considerations associated with each kind of source.
///
/// When a `Translator` is used more than once to translate different inputs, it outputs the
/// logical concatenation of all documents from all inputs as if they had been presented in a
/// single input. When translating to a format without multi-document support, translation fails if
/// the translator encounters more than one document in the first input, or if the translator is
/// called a second time with another input.
pub struct Translator<W>(Dispatcher<W>)
where
	W: Write;

impl<W> Translator<W>
where
	W: Write,
{
	/// Creates a translator that produces output in the given format.
	pub fn new(output: W, to: Format) -> Translator<W> {
		Translator(Dispatcher::new(output, to))
	}

	/// Translates the contents of a single input slice to a different format.
	///
	/// Slices are typically more efficient to translate than readers, but require all input to be
	/// available in memory. For unbounded streams like standard input or non-regular files,
	/// consider [`translate_reader`] rather than manually buffering the entire reader.
	///
	/// When `from` is `None`, the translator attempts to detect the format from the input itself.
	pub fn translate_slice(&mut self, input: &[u8], from: Option<Format>) -> Result<()> {
		self.translate(input::Handle::from_slice(input), from)
	}

	/// Translates the contents of a single reader to a different format.
	///
	/// Reader inputs enable streaming translation for most formats, where the translator handles
	/// multiple documents without buffering more than one in memory at a time. When translating
	/// from a format without streaming support, the translator automatically buffers the entire
	/// input.
	///
	/// When `from` is `None`, the translator attempts to detect the format from the input itself.
	/// The current implementation must buffer at least one full document to perform the detection
	/// before starting translation.
	pub fn translate_reader<R>(&mut self, input: R, from: Option<Format>) -> Result<()>
	where
		R: Read,
	{
		self.translate(input::Handle::from_reader(input), from)
	}

	/// Translates a single serialized input to a different format.
	fn translate(&mut self, mut input: input::Handle<'_>, from: Option<Format>) -> Result<()> {
		let from = match from {
			Some(format) => format,
			None => match Format::detect(&mut input)? {
				Some(format) => format,
				None => return Err("unable to detect input format".into()),
			},
		};
		match from {
			Format::Json => json::transcode(input, &mut self.0),
			Format::Msgpack => msgpack::transcode(input, &mut self.0),
			Format::Toml => toml::transcode(input, &mut self.0),
			Format::Yaml => yaml::transcode(input, &mut self.0),
		}
	}

	/// [Flushes](Write::flush) the underlying writer.
	pub fn flush(&mut self) -> io::Result<()> {
		(&mut self.0).flush()
	}
}

/// A trait for output formats to receive their translatable input.
trait Output {
	fn transcode_from<'de, D, E>(&mut self, de: D) -> Result<()>
	where
		D: de::Deserializer<'de, Error = E>,
		E: de::Error + Send + Sync + 'static;

	fn transcode_value<S>(&mut self, value: S) -> Result<()>
	where
		S: ser::Serialize;

	fn flush(&mut self) -> io::Result<()>;
}

/// An [`Output`] supporting static dispatch based on a known output format.
enum Dispatcher<W>
where
	W: Write,
{
	Json(json::Output<W>),
	Msgpack(msgpack::Output<W>),
	Toml(toml::Output<W>),
	Yaml(yaml::Output<W>),
}

impl<W> Dispatcher<W>
where
	W: Write,
{
	fn new(writer: W, to: Format) -> Dispatcher<W> {
		match to {
			Format::Json => Dispatcher::Json(json::Output::new(writer)),
			Format::Msgpack => Dispatcher::Msgpack(msgpack::Output::new(writer)),
			Format::Toml => Dispatcher::Toml(toml::Output::new(writer)),
			Format::Yaml => Dispatcher::Yaml(yaml::Output::new(writer)),
		}
	}
}

impl<W> Output for &mut Dispatcher<W>
where
	W: Write,
{
	fn transcode_from<'de, D, E>(&mut self, de: D) -> Result<()>
	where
		D: de::Deserializer<'de, Error = E>,
		E: de::Error + Send + Sync + 'static,
	{
		match self {
			Dispatcher::Json(output) => output.transcode_from(de),
			Dispatcher::Msgpack(output) => output.transcode_from(de),
			Dispatcher::Toml(output) => output.transcode_from(de),
			Dispatcher::Yaml(output) => output.transcode_from(de),
		}
	}

	fn transcode_value<S>(&mut self, value: S) -> Result<()>
	where
		S: ser::Serialize,
	{
		match self {
			Dispatcher::Json(output) => output.transcode_value(value),
			Dispatcher::Msgpack(output) => output.transcode_value(value),
			Dispatcher::Toml(output) => output.transcode_value(value),
			Dispatcher::Yaml(output) => output.transcode_value(value),
		}
	}

	fn flush(&mut self) -> io::Result<()> {
		match self {
			Dispatcher::Json(output) => output.flush(),
			Dispatcher::Msgpack(output) => output.flush(),
			Dispatcher::Toml(output) => output.flush(),
			Dispatcher::Yaml(output) => output.flush(),
		}
	}
}

/// The set of input and output formats supported by xt.
///
/// Support for each format comes largely from external crates, with some additional preprocessing
/// by xt for select formats. The crate selection for each format is **not stable**,
/// and is documented for informational purposes only.
#[derive(Copy, Clone)]
#[non_exhaustive]
pub enum Format {
	/// The [JSON][json] format as interpreted by [`serde_json`].
	///
	/// This format supports multi-document translation and streaming input.
	///
	/// [json]: https://datatracker.ietf.org/doc/html/rfc8259
	Json,
	/// The [MessagePack][msgpack] format as interpreted by [`rmp_serde`].
	///
	/// This format supports multi-document translation and streaming input.
	///
	/// [msgpack]: https://msgpack.org/
	Msgpack,
	/// The [TOML][toml] format as interpreted by [`toml`][::toml].
	///
	/// This format supports single-document translation only,
	/// and as such does not support streaming input.
	///
	/// [toml]: https://github.com/toml-lang/toml
	Toml,
	/// The [YAML 1.2][yaml] format as interpreted by [`serde_yaml`].
	///
	/// This format supports multi-document translation and streaming input.
	///
	/// [yaml]: https://yaml.org/spec/1.2.2/
	Yaml,
}

impl fmt::Display for Format {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.write_str(match self {
			Self::Json => "JSON",
			Self::Msgpack => "MessagePack",
			Self::Toml => "TOML",
			Self::Yaml => "YAML",
		})
	}
}

impl Format {
	/// Detects the input format by trying to parse a single document with each one.
	fn detect(input: &mut input::Handle) -> io::Result<Option<Format>> {
		// As a binary format, we expect MessagePack to be more restrictive than any text format.
		// Detection of MessagePack inputs is limited to collection types; see comments in the
		// implementation for details.
		if crate::msgpack::input_matches(input.borrow_mut())? {
			return Ok(Some(Format::Msgpack));
		}

		// We expect JSON to be more restrictive than other text formats. For example, a "#"
		// comment at the start of a document could be TOML or YAML, but definitely not JSON.
		if crate::json::input_matches(input.borrow_mut())? {
			return Ok(Some(Format::Json));
		}

		// YAML is _less_ restrictive than TOML, but we want to try it first since it supports
		// streaming input (so detection may require less buffering). Detection of YAML inputs is
		// limited to collection types; see comments in the implementation for details.
		if crate::yaml::input_matches(input.borrow_mut())? {
			return Ok(Some(Format::Yaml));
		}

		// TOML is the only format that must fully buffer its input, and imposes its own limits to
		// avoid unbounded memory consumption.
		if crate::toml::input_matches(input.borrow_mut())? {
			return Ok(Some(Format::Toml));
		}

		Ok(None)
	}
}

/// Cast the offset of a memory-based [`io::Read`] to a [`usize`].
///
/// While `Read` APIs present offsets as `u64`s, any offset into a reader over an in-memory slice
/// must naturally be representable in a width that covers every possible memory address.
///
/// # Panics
///
/// When debug assertions are enabled and `n` doesn't fit in a usize.
/// This is a tradeoff between the relative efficiency and potential dangers of plain `as` casts.
#[inline(always)]
#[allow(clippy::cast_possible_truncation)]
fn cast_read_offset_usize(n: u64) -> usize {
	if cfg!(debug_assertions) {
		usize::try_from(n).expect("reader offset should fit in a usize")
	} else {
		n as usize
	}
}
