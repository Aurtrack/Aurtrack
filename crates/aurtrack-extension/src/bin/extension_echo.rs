use std::io;

use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let request: Value = serde_json::from_reader(io::stdin())?;
    let response = json!({
        "extension": request.get("extension").cloned().unwrap_or(Value::Null),
        "command": request.get("command").cloned().unwrap_or(Value::Null),
        "echo": request.get("args").cloned().unwrap_or(Value::Null),
    });
    println!("{response}");
    Ok(())
}
