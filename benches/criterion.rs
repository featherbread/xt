use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};

use xt::Format;

criterion_main!(small, medium);

criterion_group! {
	name = small;
	config = Criterion::default();
	targets = small_json, small_yaml, small_toml, small_msgpack
}

criterion_group! {
	name = medium;
	config = Criterion::default().measurement_time(Duration::from_secs(20));
	targets = medium_json, medium_yaml, medium_toml, medium_msgpack
}

macro_rules! xt_benchmark {
	(
		name = $name:ident;
		sources = $($source:ident),+;
		loader = $loader:path;
		translation = $from:path => $to:path;
		$(group_config { $($setting_name:ident = $setting_value:expr;)* })?
	) => {
		fn $name(c: &mut Criterion) {
			let mut group = c.benchmark_group(stringify!($name));
			let input = $loader($from);

			$($(group.$setting_name($setting_value);)*)?

			$(group.bench_function(stringify!($source), |b| {
				b.iter(|| {
					xt_benchmark!(@translate_fn $source)(
						&*input,
						black_box(Some($from)),
						black_box($to),
						std::io::sink(),
					)
				})
			});)+

			group.finish();
		}
	};
	(@translate_fn buffer) => { xt::translate_slice };
	(@translate_fn reader) => { xt::translate_reader };
}

xt_benchmark! {
	name = small_json;
	sources = buffer, reader;
	loader = load_small_data;
	translation = Format::Json => Format::Msgpack;
}

xt_benchmark! {
	name = small_yaml;
	sources = buffer, reader;
	loader = load_small_data;
	translation = Format::Yaml => Format::Json;
}

xt_benchmark! {
	name = small_toml;
	sources = buffer;
	loader = load_small_data;
	translation = Format::Toml => Format::Json;
}

xt_benchmark! {
	name = small_msgpack;
	sources = buffer, reader;
	loader = load_small_data;
	translation = Format::Msgpack => Format::Json;
}

fn load_small_data(format: Format) -> Vec<u8> {
	let input: &[u8] = include_bytes!("k8s-job.yaml");

	let mut output = Vec::with_capacity(512);
	xt::translate_slice(input, Some(Format::Yaml), format, &mut output)
		.expect("k8s-job.yaml should be valid YAML");

	output
}

xt_benchmark! {
	name = medium_json;
	sources = buffer, reader;
	loader = load_medium_data;
	translation = Format::Json => Format::Msgpack;
}

xt_benchmark! {
	name = medium_yaml;
	sources = buffer, reader;
	loader = load_medium_data;
	translation = Format::Yaml => Format::Json;
}

xt_benchmark! {
	name = medium_toml;
	sources = buffer;
	loader = load_medium_data;
	translation = Format::Toml => Format::Json;
}

xt_benchmark! {
	name = medium_msgpack;
	sources = buffer, reader;
	loader = load_medium_data;
	translation = Format::Msgpack => Format::Json;
}

fn load_medium_data(format: Format) -> Vec<u8> {
	use rmp::Marker;

	// These manifests were generated using a `helm template` command that should be reproducible
	// given the correct version of the original chart.
	let input: &[u8] = include_bytes!("k8s-kyverno.yaml");

	// For TOML compatibility, we need to take this stream of Kubernetes manifests and put them
	// into a single object. Since MessagePack doesn't use characters or indentation for structure,
	// it's (surprisingly) the easiest way I can think to do this.
	let mut packed = Vec::new();

	packed.push(Marker::FixMap(1).to_u8()); // Map of 1 element; key and value follow.

	// Key, string of 9 characters.
	let key = b"manifests";
	let len = u8::try_from(key.len()).unwrap();
	packed.push(Marker::FixStr(len).to_u8());
	packed.extend(key);

	// Array of 79 elements (`xt k8s-kyverno.yaml | jq -s length`); elements follow.
	let len: u16 = 79;
	packed.push(Marker::Array16.to_u8());
	packed.extend(len.to_be_bytes());

	// The elements of the array.
	xt::translate_slice(input, Some(Format::Yaml), Format::Msgpack, &mut packed)
		.expect("k8s-kyverno.yaml should be valid YAML");

	// Now, translate that {"manifests": [...]} object to the final output format.
	let mut output = Vec::new();
	xt::translate_slice(&packed, Some(Format::Msgpack), format, &mut output)
		.expect("packed object should be valid");

	output
}
