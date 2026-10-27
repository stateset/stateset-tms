use stateset_freight_storage_codec::{pack, unpack, Config, Packed};
use std::io::{self, Read, Write};

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().ok_or("pack|unpack required")?;
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| e.to_string())?;
    let output = match mode.as_str() {
        "pack" => {
            let span = args
                .next()
                .unwrap_or_else(|| "64".into())
                .parse()
                .map_err(|_| "Invalid span")?;
            let target_bytes = args
                .next()
                .unwrap_or_else(|| "65536".into())
                .parse()
                .map_err(|_| "Invalid byte target")?;
            serde_json::to_string(&pack(&input, Config { span, target_bytes })?)
                .map_err(|e| e.to_string())?
        }
        "unpack" => {
            let frame = serde_json::from_str::<Packed>(&input).map_err(|e| e.to_string())?;
            drop(input);
            unpack(frame)?
        }
        _ => return Err("Unsupported codec command".into()),
    };
    if args.next().is_some() {
        return Err("Unexpected codec argument".into());
    }
    io::stdout()
        .write_all(output.as_bytes())
        .map_err(|e| e.to_string())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
