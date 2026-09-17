//! Native wire-v1 fixture, built by the integration test with the host rustc.
//! No interpreter or Unix executable-bit dependency.
use std::io::{self, BufRead, Write};

fn main() {
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        let Some((_, tail)) = line.split_once("\"id\":") else {
            continue;
        };
        let id: String = tail
            .trim_start()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let result = if line.contains("plugin/manifest") {
            r#"{"name":"echo","version":"0.1.0","tools":["echo"]}"#
        } else if line.contains("tool/call") {
            r#"{"content":"hi","is_error":false}"#
        } else {
            continue;
        };
        println!("{{\"id\":{id},\"result\":{result}}}");
        io::stdout().flush().unwrap();
    }
}
