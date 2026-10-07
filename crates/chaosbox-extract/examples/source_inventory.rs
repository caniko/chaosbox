//! Metadata-only admission driver; no database, source capture, or extraction.
//! Usage: `source_inventory ROOT MAX_ENTRIES TIMEOUT_MS [SOURCE_PATH ...]`

use std::{path::Path, time::Duration};

use chaosbox_extract::{source_inventory, InventoryLimits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        return Err("usage: source_inventory ROOT MAX_ENTRIES TIMEOUT_MS [SOURCE_PATH ...]".into());
    }
    let result = source_inventory(
        Path::new(&args[0]),
        &args[3..],
        InventoryLimits {
            maximum_entries: args[1].parse()?,
            timeout: Duration::from_millis(args[2].parse()?),
        },
    )?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
