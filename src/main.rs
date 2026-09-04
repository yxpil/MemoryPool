mod model;
mod params;
mod search;
mod serve;
mod store;

use std::io::{IsTerminal, Read};
use std::process::ExitCode;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use serde_json::Value;

use model::Memory;
use params::{get_f32, get_string, get_tags, get_usize};
use store::Store;

/// Shared, persistent memory pool for AI agents around the BIT ecosystem.
#[derive(Parser, Debug)]
#[command(
    name = "memorypool",
    version,
    about,
    after_help = "Data is stored in ~/.memorypool/memories.jsonl (override the directory with MEMORYPOOL_DATA_DIR). When stdin is a pipe, a JSON object may be sent and its fields override CLI arguments (BIT exec-mode contract)."
)]
struct Cli {
    /// Emit pure machine-readable JSON on stdout
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Store a new memory and print it as JSON
    Add {
        /// Memory text
        text: Option<String>,
        /// Comma-separated tags
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// Importance in [0.0, 1.0] (default 0.5)
        #[arg(long)]
        importance: Option<f32>,
        /// Origin of the memory (default "cli")
        #[arg(long)]
        source: Option<String>,
    },
    /// Keyword search ranked by match, importance and recency
    Search {
        /// Search query (space-separated terms)
        query: Option<String>,
        /// Maximum number of results
        #[arg(long)]
        limit: Option<usize>,
        /// Only memories carrying this tag
        #[arg(long)]
        tag: Option<String>,
    },
    /// List memories, newest first
    List {
        /// Maximum number of results
        #[arg(long)]
        limit: Option<usize>,
        /// Only memories carrying this tag
        #[arg(long)]
        tag: Option<String>,
    },
    /// Fetch one memory by id
    Get {
        /// Memory id
        id: Option<String>,
    },
    /// Delete one memory by id
    Delete {
        /// Memory id
        id: Option<String>,
    },
    /// Add/remove tags on a memory
    Tag {
        /// Memory id
        id: Option<String>,
        /// Comma-separated tags to add
        #[arg(long, value_delimiter = ',')]
        add: Vec<String>,
        /// Comma-separated tags to remove
        #[arg(long, value_delimiter = ',')]
        remove: Vec<String>,
    },
    /// Show pool statistics (count, tag distribution, oldest/newest)
    Stats,
    /// Run the HTTP server (default 127.0.0.1:8751)
    Serve {
        /// Bind address
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port (0 lets the OS pick a free port)
        #[arg(long, default_value_t = 8751)]
        port: u16,
        /// Require "Authorization: Bearer <token>" on every endpoint except /health
        #[arg(long)]
        token: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // BIT exec-mode contract: when stdin is a pipe, read a JSON object and
    // let its fields override the subcommand arguments (stdin wins).
    let stdin = if matches!(cli.command, Command::Serve { .. }) {
        None
    } else {
        read_piped_stdin()
    };
    let json_mode = cli.json || stdin.is_some();
    match run(cli, stdin, json_mode) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn read_piped_stdin() -> Option<serde_json::Map<String, Value>> {
    if std::io::stdin().is_terminal() {
        return None;
    }
    let mut buf = String::new();
    match std::io::stdin().read_to_string(&mut buf) {
        Ok(0) | Err(_) => None,
        Ok(_) => match serde_json::from_str::<Value>(buf.trim()) {
            Ok(Value::Object(map)) => Some(map),
            Ok(_) => {
                eprintln!("warning: stdin did not contain a JSON object; ignoring it");
                None
            }
            Err(_) => {
                eprintln!("warning: stdin did not contain valid JSON; ignoring it");
                None
            }
        },
    }
}

fn run(cli: Cli, stdin: Option<serde_json::Map<String, Value>>, json_mode: bool) -> Result<()> {
    match cli.command {
        Command::Add {
            text,
            tags,
            importance,
            source,
        } => {
            let store = Store::open()?;
            let merged = merge_add(text, tags, importance, source, stdin.as_ref())?;
            let memory = model::new_memory(merged.0, merged.1, merged.2, merged.3);
            let stored = store.add(memory)?;
            print_memory(&stored, json_mode);
        }
        Command::Search { query, limit, tag } => {
            let store = Store::open()?;
            let query = get_string_or_arg(&stdin, &["query", "q"], query, "search query")?;
            let limit = get_usize(&stdin_map(&stdin), &["limit"])
                .or(limit)
                .unwrap_or(10);
            let tag = get_string(&stdin_map(&stdin), &["tag"]).or(tag);
            let results = search::search(&store.list(None), &query, tag.as_deref(), limit);
            if json_mode {
                print_value(&results_json(&results), true);
            } else if results.is_empty() {
                println!("No memories found.");
            } else {
                for (rank, (m, score)) in results.iter().enumerate() {
                    println!(
                        "{}. [{}] {} {}",
                        rank + 1,
                        format_score(*score),
                        short_id(&m.id),
                        m.text
                    );
                    print_meta(m);
                }
            }
        }
        Command::List { limit, tag } => {
            let store = Store::open()?;
            let limit = get_usize(&stdin_map(&stdin), &["limit"]).or(limit);
            let tag = get_string(&stdin_map(&stdin), &["tag"]).or(tag);
            let mut items = store.list(tag.as_deref());
            if let Some(limit) = limit {
                items.truncate(limit);
            }
            if json_mode {
                let arr = Value::Array(
                    items
                        .iter()
                        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
                        .collect(),
                );
                print_value(&arr, true);
            } else if items.is_empty() {
                println!("No memories found.");
            } else {
                for m in &items {
                    println!("{} {} {}", short_id(&m.id), m.created_at, m.text);
                    print_meta(m);
                }
            }
        }
        Command::Get { id } => {
            let store = Store::open()?;
            let id = get_string_or_arg(&stdin, &["id"], id, "memory id")?;
            match store.get(&id) {
                Some(m) => print_memory(&m, json_mode),
                None => bail!("memory {id} not found"),
            }
        }
        Command::Delete { id } => {
            let store = Store::open()?;
            let id = get_string_or_arg(&stdin, &["id"], id, "memory id")?;
            if store.delete(&id)? {
                if json_mode {
                    print_value(&serde_json::json!({"deleted": id}), true);
                } else {
                    println!("Deleted {id}");
                }
            } else {
                bail!("memory {id} not found");
            }
        }
        Command::Tag { id, add, remove } => {
            let store = Store::open()?;
            let id = get_string_or_arg(&stdin, &["id"], id, "memory id")?;
            let add = get_tags(&stdin_map(&stdin), &["add", "tags"]).unwrap_or(add);
            let remove = get_tags(&stdin_map(&stdin), &["remove"]).unwrap_or(remove);
            match store.set_tags(&id, add, remove)? {
                Some(m) => {
                    print_memory(&m, json_mode);
                    if !json_mode {
                        println!(
                            "Tags: {}",
                            if m.tags.is_empty() {
                                "(none)".to_string()
                            } else {
                                m.tags.join(", ")
                            }
                        );
                    }
                }
                None => bail!("memory {id} not found"),
            }
        }
        Command::Stats => {
            let store = Store::open()?;
            let stats = store.stats();
            if json_mode {
                print_value(&serde_json::to_value(&stats).unwrap_or(Value::Null), true);
            } else {
                println!("Count: {}", stats.count);
                let tags = stats
                    .tags
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                println!(
                    "Tags: {}",
                    if tags.is_empty() {
                        "(none)".to_string()
                    } else {
                        tags
                    }
                );
                println!("Oldest: {}", stats.oldest.as_deref().unwrap_or("(none)"));
                println!("Newest: {}", stats.newest.as_deref().unwrap_or("(none)"));
            }
        }
        Command::Serve { host, port, token } => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(serve::run(host, port, token))?;
        }
    }
    Ok(())
}

type AddInput = (String, Vec<String>, f32, Option<String>);

fn merge_add(
    text: Option<String>,
    tags: Vec<String>,
    importance: Option<f32>,
    source: Option<String>,
    stdin: Option<&serde_json::Map<String, Value>>,
) -> Result<AddInput> {
    let map = stdin.cloned().unwrap_or_default();
    let text = get_string(&map, &["text", "value"]).or(text);
    let Some(text) = text else {
        bail!("missing required field: text (pass as argument or via piped stdin JSON)");
    };
    let tags = get_tags(&map, &["tags"]).unwrap_or(tags);
    let importance = get_f32(&map, &["importance"]).or(importance).unwrap_or(0.5);
    let source = get_string(&map, &["source"]).or(source);
    Ok((text, tags, importance, source))
}

fn stdin_map(stdin: &Option<serde_json::Map<String, Value>>) -> serde_json::Map<String, Value> {
    stdin.clone().unwrap_or_default()
}

fn get_string_or_arg(
    stdin: &Option<serde_json::Map<String, Value>>,
    keys: &[&str],
    arg: Option<String>,
    what: &str,
) -> Result<String> {
    let from_stdin = get_string(&stdin_map(stdin), keys);
    from_stdin.or(arg).ok_or_else(|| {
        anyhow::anyhow!("missing required {what} (pass as argument or via piped stdin JSON)")
    })
}

fn print_memory(m: &Memory, json_mode: bool) {
    print_value(&serde_json::to_value(m).unwrap_or(Value::Null), json_mode);
}

fn print_value(v: &Value, json_mode: bool) {
    if json_mode {
        println!("{v}");
    } else {
        println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
    }
}

fn results_json(results: &[(Memory, f64)]) -> Value {
    Value::Array(
        results
            .iter()
            .map(|(m, score)| {
                let mut v = serde_json::to_value(m).unwrap_or(Value::Null);
                if let Some(obj) = v.as_object_mut() {
                    obj.insert(
                        "score".into(),
                        serde_json::json!((*score * 1000.0).round() / 1000.0),
                    );
                }
                v
            })
            .collect(),
    )
}

fn short_id(id: &str) -> &str {
    id.split('-').next().unwrap_or(id)
}

fn format_score(score: f64) -> String {
    format!("{score:.3}")
}

fn print_meta(m: &Memory) {
    if !m.tags.is_empty() {
        println!(
            "    tags: {} importance: {:.2} source: {}",
            m.tags.join(", "),
            m.importance,
            m.source
        );
    }
}
