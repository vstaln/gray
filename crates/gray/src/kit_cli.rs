use std::io::{BufRead, BufReader, IsTerminal, Read, Write};

use serde_json::{Value, json};

const USAGE: &str = "Usage:
  kit                       list available kit tools
  kit --list                list available kit tools
  kit search <query>        search tools by keyword
  kit <tool> --help         show tool flags and parameter schema
  kit <tool> [FLAGS]        run a tool (--key val, --flag, --json '{...}', or stdin JSON)
  kit <server> <tool> ...   run an MCP server tool";

fn send_request(req: &Value) -> anyhow::Result<Value> {
    let sock = std::env::var("GRAY_KIT_SOCK")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("GRAY_KIT_SOCK is not set — run `kit` inside a live gray session")
        })?;
    let mut payload = req.clone();
    if let Ok(tok) = std::env::var("GRAY_KIT_TOKEN")
        && let Some(obj) = payload.as_object_mut()
    {
        obj.insert("token".to_string(), Value::String(tok));
    }
    let mut line = serde_json::to_vec(&payload)?;
    line.push(b'\n');

    #[cfg(unix)]
    let raw = {
        use std::os::unix::net::UnixStream;
        let mut stream = UnixStream::connect(&sock)?;
        stream.write_all(&line)?;
        stream.flush()?;
        let mut reader = BufReader::new(stream);
        let mut resp = String::new();
        reader.read_line(&mut resp)?;
        resp
    };

    #[cfg(not(unix))]
    let raw = {
        let mut stream = std::net::TcpStream::connect(&sock)?;
        stream.write_all(&line)?;
        stream.flush()?;
        let mut reader = BufReader::new(stream);
        let mut resp = String::new();
        reader.read_line(&mut resp)?;
        resp
    };

    if raw.trim().is_empty() {
        anyhow::bail!("kit host closed connection without a reply");
    }
    Ok(serde_json::from_str(&raw)?)
}

fn read_piped_stdin() -> Option<String> {
    if std::io::stdin().is_terminal() {
        return None;
    }
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_ok() && !buf.trim().is_empty() {
        Some(buf)
    } else {
        None
    }
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let first = args.first().map(String::as_str);
    if matches!(first, Some("-h") | Some("--help")) {
        println!("{USAGE}");
        return Ok(());
    }
    let req = match first {
        None | Some("--list") | Some("list") => json!({"action": "list"}),
        Some("search") => {
            let query = args[1..].join(" ");
            json!({"action": "search", "target": query})
        }
        Some("help") | Some("open") => {
            if args.len() < 2 {
                println!("{USAGE}");
                return Ok(());
            }
            json!({"action": "help", "argv": &args[1..]})
        }
        Some(_) => {
            if args.iter().any(|a| a == "--help" || a == "-h") {
                let target_argv: Vec<String> = args
                    .iter()
                    .take_while(|a| *a != "--help" && *a != "-h")
                    .cloned()
                    .collect();
                json!({"action": "help", "argv": target_argv})
            } else {
                let mut req = json!({"action": "run", "argv": args});
                if let Some(stdin_text) = read_piped_stdin() {
                    req["stdin"] = Value::String(stdin_text);
                }
                req
            }
        }
    };

    let reply = send_request(&req)?;
    let ok = reply.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if let Some(text) = reply.get("text").and_then(Value::as_str) {
        print!("{text}");
        if !text.ends_with('\n') {
            println!();
        }
        return Ok(());
    }
    if let Some(content) = reply.get("content").and_then(Value::as_str) {
        if ok {
            print!("{content}");
            if !content.ends_with('\n') {
                println!();
            }
            return Ok(());
        }
        eprintln!("{content}");
        std::process::exit(1);
    }
    if let Some(err) = reply.get("error").and_then(Value::as_str) {
        eprintln!("{err}");
        std::process::exit(1);
    }
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}
