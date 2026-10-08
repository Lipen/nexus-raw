//! Generate a deterministic synthetic tree by hand.
//!
//! The bytes are the ones the benches build: same module, same seed.
//!
//! ```text
//! cargo run --manifest-path bench/Cargo.toml --bin gen-tree -- --out /tmp/nxr-tree --files 500
//! ```

use std::path::Path;
use std::process::ExitCode;

use nexus_raw_bench::TreeSpec;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("gen-tree: {msg}");
            eprintln!("usage: gen-tree --out DIR [--files N] [--depth N] [--fanout N] [--size N] [--seed N]");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let mut spec = TreeSpec::standard();
    let mut out: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = Some(args.next().ok_or("--out wants a directory")?),
            "--files" => spec.files = next_num(&mut args, "--files")?,
            "--depth" => spec.depth = next_num(&mut args, "--depth")?,
            "--fanout" => spec.fanout = next_num(&mut args, "--fanout")?,
            "--size" => spec.size = next_num(&mut args, "--size")?,
            "--seed" => spec.seed = next_num(&mut args, "--seed")?,
            other => return Err(format!("unknown flag {other:?}")),
        }
    }
    let out = out.ok_or("--out is required: where the tree goes")?;
    let root = Path::new(&out);
    nexus_raw_bench::write_tree_at(root, spec)
        .map_err(|e| format!("cannot write {}: {e}", root.display()))?;
    let total = u64::from(spec.files) * u64::from(spec.size);
    println!(
        "{}: {} files, {} bytes, seed {}",
        root.display(),
        spec.files,
        total,
        spec.seed
    );
    Ok(())
}

fn next_num<T: std::str::FromStr>(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<T, String>
where
    T::Err: std::fmt::Display,
{
    let raw = args.next().ok_or(format!("{flag} wants a value"))?;
    raw.parse()
        .map_err(|e| format!("{flag}: {raw:?} is not a valid number: {e}"))
}
