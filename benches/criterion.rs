use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

use xt::Format;

criterion_main!(small);

criterion_group! {
	name = small;
	config = Criterion::default();
	targets = small_json, small_yaml, small_toml, small_msgpack
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
