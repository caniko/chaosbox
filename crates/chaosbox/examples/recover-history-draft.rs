//! One-shot migration adapter: read a normalized message from stdin, emit the
//! exact original and reversible projection. No source/runtime database access.
use std::io::Read;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let snapshot = args.next().ok_or("source snapshot digest required")?;
    let settled_at: i64 = args
        .next()
        .ok_or("journaled settlement milliseconds required")?
        .parse()?;
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err("record exceeds 32 MiB bound".into());
    }
    let original = serde_json::from_slice(&bytes)?;
    let projection = chaosbox::history::recover_draft(original, &snapshot, settled_at)?;
    println!("{}", serde_json::to_string(&projection)?);
    Ok(())
}
