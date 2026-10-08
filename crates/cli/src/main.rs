//! `xdl` — the xitter-dl command line.
//!
//! Exists so the entire pipeline can be dogfooded from a terminal before the
//! GUI exists, and so the importers have a test harness that is not a webview
//! (PRD §7.2). Argument parsing is hand-rolled: four subcommands do not
//! justify a dependency, and keeping this crate's tree small keeps the
//! `crates/core` build honest.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use xdl_core::{db, import, search};

const USAGE: &str = "\
xdl — xitter-dl command line

USAGE:
    xdl <COMMAND> [OPTIONS]

COMMANDS:
    import <PATH|->      Import a JSON payload, NDJSON file, HAR, or a directory.
                         Use `-` to read a single JSON payload from stdin, which
                         is the quickest way to import one pasted tweet.
    search <QUERY...>    Full-text search across the library.
    show <ID>            Print one post in full.
    stats                Counts for the library.
    paths                Print where the database lives.

OPTIONS:
    --db <PATH>          Use a specific database file.
    --limit <N>          Max results (search). Default 20.
    --json               Machine-readable output.
    -h, --help           This message.

ENVIRONMENT:
    XITTER_DL_DB     Overrides the default database location.
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // `{:#}` prints the whole anyhow context chain on one line, which
            // is what a CLI error should be.
            eprintln!("xdl: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return Ok(());
    }

    let mut db_path: Option<PathBuf> = None;
    let mut limit: usize = 20;
    let mut json = false;
    let mut positional: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--db" => {
                i += 1;
                db_path = Some(PathBuf::from(
                    args.get(i).context("--db needs a path")?,
                ));
            }
            "--limit" => {
                i += 1;
                limit = args
                    .get(i)
                    .context("--limit needs a number")?
                    .parse()
                    .context("--limit must be a number")?;
            }
            "--json" => json = true,
            other => positional.push(other.to_owned()),
        }
        i += 1;
    }

    let Some(command) = positional.first().cloned() else {
        print!("{USAGE}");
        return Ok(());
    };
    let rest = &positional[1..];

    let path = db_path.unwrap_or_else(xdl_core::default_library_path);
    // Opening creates and migrates. That means `xdl stats` on a fresh machine
    // makes an empty library rather than erroring, which is the friendlier
    // behaviour for a first run.
    let mut lib = db::Library::open(&path)
        .with_context(|| format!("opening database at {}", path.display()))?;

    match command.as_str() {
        "import" => cmd_import(&mut lib, rest, json),
        "search" => cmd_search(&lib, rest, limit, json),
        "show" => cmd_show(&lib, rest, json),
        "stats" => cmd_stats(&lib, json),
        "paths" => {
            println!("{}", path.display());
            Ok(())
        }
        other => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

fn cmd_import(lib: &mut db::Library, rest: &[String], json: bool) -> Result<()> {
    let target = rest.first().context("import needs a path (or `-` for stdin)")?;
    let now = xdl_core::now();

    // One transaction for the whole import: an order of magnitude faster than
    // committing per record, and it means a failed import leaves no partial
    // state to reason about.
    let tx = lib.conn_mut().transaction()?;

    let summary = if target == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .context("reading stdin")?;
        import::payload::import_json_text(&tx, &text, None, now)?
    } else {
        let path = PathBuf::from(target);
        if !path.exists() {
            bail!("no such file or directory: {}", path.display());
        }
        import::import_path(&tx, &path, now)?
    };

    tx.commit()?;

    let stats = db::read::stats(lib.conn())?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "files": summary.files,
                "seen": summary.stats.seen,
                "new": summary.stats.new,
                "updated": summary.stats.updated,
                "payloads": summary.stats.payloads,
                "problems": summary.problems,
                "library": stats,
            })
        );
        return Ok(());
    }

    println!(
        "imported {} file(s): {} seen, {} new, {} already present",
        summary.files, summary.stats.seen, summary.stats.new, summary.stats.updated
    );
    for p in &summary.problems {
        // Warnings, not failures — the import as a whole succeeded.
        eprintln!("  warning: {p}");
    }
    println!(
        "library now holds {} bookmarks across {} posts",
        stats.bookmarks, stats.posts
    );
    Ok(())
}

fn cmd_search(lib: &db::Library, rest: &[String], limit: usize, json: bool) -> Result<()> {
    let query = rest.join(" ");
    if query.trim().is_empty() {
        bail!("search needs a query");
    }

    // The CLI asks for printable delimiters; the library default is control
    // characters, which exist so the frontend never has to parse HTML.
    let hits = search::search_with(lib.conn(), &query, limit, "[", "]")?;

    if json {
        println!("{}", serde_json::to_string_pretty(&hits)?);
        return Ok(());
    }

    if hits.is_empty() {
        println!("no matches for {query:?}");
        return Ok(());
    }

    for hit in &hits {
        let t = &hit.post.tweet;
        let when = t
            .created_at
            .map(xdl_core::x::format_local)
            .unwrap_or_else(|| "unknown date".into());
        println!("{}  @{}  {}", t.id, t.author.handle, when);
        // Snippet when we have one; it is what makes the CLI search useful.
        match &hit.snippet {
            Some(s) => println!("    {}", s.replace('\n', " ")),
            None => println!("    {}", first_line(&t.text)),
        }
        println!("    [{}]", hit.matched.join("+"));
    }
    println!("\n{} result(s)", hits.len());
    Ok(())
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 120 {
        let cut: String = line.chars().take(117).collect();
        format!("{cut}...")
    } else {
        line.to_owned()
    }
}

fn cmd_show(lib: &db::Library, rest: &[String], json: bool) -> Result<()> {
    let id = rest.first().context("show needs a post ID")?;
    let post = db::read::get_post(lib.conn(), id)?;
    let Some(post) = post else {
        bail!("no post with id {id}");
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&post)?);
        return Ok(());
    }

    let t = &post.tweet;
    println!("{} (@{})  {}", t.author.name, t.author.handle, t.id);
    if let Some(ts) = t.created_at {
        println!("{}", xdl_core::x::format_local(ts));
    }
    println!();
    println!("{}", t.text);
    println!();

    if let Some(q) = &t.quoted {
        println!("  ┌ quoting @{} — {}", q.author.handle, first_line(&q.text));
    }
    if let Some(c) = &t.card {
        println!(
            "  ▣ card: {} — {}",
            c.title.as_deref().unwrap_or("(untitled)"),
            c.domain.as_deref().unwrap_or("")
        );
    }
    for m in &t.media {
        println!(
            "  ▢ {:?} {}x{} {}",
            m.kind,
            m.width.unwrap_or(0),
            m.height.unwrap_or(0),
            m.alt_text.as_deref().unwrap_or("(no alt text)")
        );
    }

    println!(
        "\n  {} likes · {} reposts · {} replies · {} views",
        t.metrics.likes,
        t.metrics.reposts,
        t.metrics.replies,
        t.metrics.views.map(|v| v.to_string()).unwrap_or_else(|| "-".into())
    );

    // The two fields most likely to be misread, so they are labelled
    // explicitly rather than printed raw (PRD §6.3).
    match post.bookmarked_at {
        Some(ts) => println!("  saved (observed): {}", xdl_core::x::format_local(ts)),
        None => println!("  saved (observed): unknown — backfill capture cannot know this"),
    }
    if post.removed_at.is_some() {
        println!("  no longer in your bookmarks");
    }
    Ok(())
}

fn cmd_stats(lib: &db::Library, json: bool) -> Result<()> {
    let s = db::read::stats(lib.conn())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&s)?);
        return Ok(());
    }
    println!("bookmarks   {}", s.bookmarks);
    println!("posts       {}  (includes quoted and enriched posts)", s.posts);
    println!("authors     {}", s.authors);
    println!("media       {}", s.media);
    println!("payloads    {}", s.payloads);
    println!("flagged     {}  (no longer in your bookmarks)", s.removed);
    Ok(())
}
