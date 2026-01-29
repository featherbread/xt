//! Handle errors in the xt binary by logging and exiting.

/// Formats a message to standard error, then terminates the current process with exit code 1.
macro_rules! die {
	($fmt:literal $(, $($args:tt)* )?) => {{
		use ::std::io::Write;
		let _ = writeln!(
			::std::io::stderr().lock(),
			"xt error: {}",
			format_args!($fmt $(, $($args)* )?),
		);
		::std::process::exit(1);
	}};
}

/// Formats a message to standard error, including the provided file path, then terminates the
/// current process with exit code 1.
macro_rules! die_in {
	($path:expr, $fmt:literal $(, $($args:tt)* )?) => {{
		use ::std::io::Write;
		let _ = writeln!(
			::std::io::stderr().lock(),
			"xt error in {}: {}",
			$path,
			format_args!($fmt $(, $($args)* )?),
		);
		::std::process::exit(1);
	}};
}
