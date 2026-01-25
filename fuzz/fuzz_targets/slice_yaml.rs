#![no_main]

use std::io;

use libfuzzer_sys::fuzz_target;

use xt::Format;

fuzz_target!(|data: &[u8]| {
	let _ = xt::translate_slice(data, Some(Format::Yaml), Format::Yaml, io::sink());
});
