# CLI Commands Guide

Define custom CLI commands with arguments, flags, and full access to the framework.

---

## Quick Start

```rust
const GREET: CommandId = CommandId::new("greet");

fn commands(reg: &mut CommandRegistry) -> Result<()> {
    reg.command(
        GREET,
        Command::new("greet").about("Say hello"),
        |inv| async move {
            println!("Hello from Forge!");
            Ok(())
        },
    )?;
    Ok(())
}
```

Register and run:

```rust
App::builder()
    .load_env()
    .load_config_dir("config")
    .register_commands(commands)
    .run_cli()?;
```

```bash
cargo run -- greet
# Hello from Forge!
```

---

## Defining Commands

Every command needs three things: a `CommandId`, a `clap::Command` definition, and an async handler.

### Simple Command (No Arguments)

```rust
const PING: CommandId = CommandId::new("ping");

reg.command(
    PING,
    Command::new("ping").about("Check if the app is alive"),
    |_inv| async move {
        println!("pong");
        Ok(())
    },
)?;
```

```bash
cargo run -- ping
```

### Command with Arguments

```rust
const IMPORT: CommandId = CommandId::new("import:users");

reg.command(
    IMPORT,
    Command::new("import:users")
        .about("Import users from a CSV file")
        .arg(Arg::new("file")
            .required(true)
            .help("Path to CSV file"))
        .arg(Arg::new("dry-run")
            .long("dry-run")
            .action(ArgAction::SetTrue)
            .help("Preview without writing to database")),
    |inv| async move {
        let file = inv.matches().get_one::<String>("file").unwrap();
        let dry_run = inv.matches().get_flag("dry-run");

        println!("Importing from: {file}");
        if dry_run {
            println!("(dry run — no changes will be made)");
        }

        // ... import logic ...

        Ok(())
    },
)?;
```

```bash
cargo run -- import:users users.csv
cargo run -- import:users users.csv --dry-run
```

### Command with Optional Arguments and Defaults

```rust
const EXPORT: CommandId = CommandId::new("export:orders");

reg.command(
    EXPORT,
    Command::new("export:orders")
        .about("Export orders to file")
        .arg(Arg::new("format")
            .long("format")
            .default_value("csv")
            .help("Output format: csv or json"))
        .arg(Arg::new("output")
            .long("output")
            .short('o')
            .default_value("exports/orders")
            .help("Output directory"))
        .arg(Arg::new("limit")
            .long("limit")
            .value_parser(clap::value_parser!(u64))
            .help("Maximum rows to export")),
    |inv| async move {
        let format = inv.matches().get_one::<String>("format").unwrap();
        let output = inv.matches().get_one::<String>("output").unwrap();
        let limit = inv.matches().get_one::<u64>("limit");

        println!("Exporting as {format} to {output}");
        if let Some(limit) = limit {
            println!("Limiting to {limit} rows");
        }

        Ok(())
    },
)?;
```

```bash
cargo run -- export:orders
cargo run -- export:orders --format json --output /tmp/orders --limit 1000
```

---

## Accessing Framework Services

The `CommandInvocation` gives you full access to the app:

```rust
const CLEANUP: CommandId = CommandId::new("cleanup:expired");

reg.command(
    CLEANUP,
    Command::new("cleanup:expired").about("Remove expired records"),
    |inv| async move {
        let app = inv.app();

        // Database
        let db = app.database()?;
        let deleted = db.raw_execute(
            "DELETE FROM sessions WHERE expires_at < NOW()",
            &[],
        ).await?;
        println!("Deleted {deleted} expired sessions");

        // Prune expired tokens
        let pruned = app.tokens()?.prune(30).await?;
        println!("Pruned {pruned} expired tokens");

        // Cache
        app.cache()?.forget("dashboard:stats").await?;

        // Jobs
        app.jobs()?.dispatch(SendCleanupReport {
            deleted_count: deleted,
        })?;

        println!("Cleanup complete");
        Ok(())
    },
)?;
```

### CommandInvocation Methods

```rust
inv.app()       // → &AppContext (database, cache, redis, email, jobs, etc.)
inv.matches()   // → &ArgMatches (clap argument values)
```

---

## Registering Commands

### In AppBuilder

```rust
App::builder()
    .register_commands(commands)
    .run_cli()?;
```

### In a ServiceProvider (for framework-level commands)

Not common for app commands — use `register_commands` on AppBuilder instead.

### In a Plugin

```rust
impl Plugin for MyPlugin {
    fn register(&self, r: &mut PluginRegistrar) -> Result<()> {
        r.register_commands(|reg| {
            reg.command(
                CommandId::new("my-plugin:status"),
                Command::new("my-plugin:status").about("Show plugin status"),
                |inv| async move {
                    println!("Plugin is running");
                    Ok(())
                },
            )?;
            Ok(())
        });
        Ok(())
    }
}
```

---

## Running Commands

```bash
# Run a specific command
cargo run -- greet

# Run with arguments
cargo run -- import:users data.csv --dry-run

# List all available commands (built-in)
cargo run -- --help
```

The `--` separates cargo arguments from your app's arguments.

### Running the CLI Kernel

The CLI kernel runs the command matching `std::env::args()` and exits:

```rust
// In main.rs — detect CLI mode
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // If first arg looks like a command, run CLI
    if args.len() > 1 && !args[1].starts_with('-') {
        return bootstrap::cli().run_cli();
    }

    // Otherwise run HTTP server
    bootstrap::http().run_http()
}
```

Or use the `PROCESS` env var pattern from the [Getting Started guide](getting-started.md).

---

## Built-in Framework Commands

These are available automatically — no registration needed:

| Command | Description |
|---------|-------------|
| `config:publish` | Generate sample config file |
| `env:publish` | Generate `.env.example` |
| `key:generate` | Generate signing + encryption keys |
| `migrate:publish` | Publish framework migration files |
| `db:migrate` | Run pending migrations |
| `db:migrate:status` | Show migration status |
| `db:rollback` | Rollback last migration batch |
| `db:seed` | Run seeders |
| `make:migration` | Create a migration file |
| `make:seeder` | Create a seeder file |
| `make:model` | Create a model file |
| `make:job` | Create a job file |
| `make:command` | Create a command file |
| `down` | Enter maintenance mode |
| `up` | Exit maintenance mode |
| `routes:list` | List named routes |
| `seed:countries` | Seed 250 countries |
| `token:prune` | Prune expired tokens |
| `plugin:list` | List plugins |
| `plugin:install-assets` | Install plugin assets |
| `plugin:scaffold` | Run plugin scaffold |
| `docs:api` | Generate API surface docs |
| `about` | Show framework version and environment |

---

## Command ID Naming Convention

Use colon-separated namespaces:

```rust
// Domain commands
CommandId::new("import:users")
CommandId::new("import:products")
CommandId::new("export:orders")
CommandId::new("cleanup:expired")

// Admin commands
CommandId::new("admin:create")
CommandId::new("admin:reset-password")

// Plugin commands
CommandId::new("my-plugin:sync")
```

The command ID must match the `clap::Command` name exactly:

```rust
reg.command(
    CommandId::new("import:users"),         // ← these must match
    Command::new("import:users"),           // ← these must match
    handler,
)?;
```

Duplicate command IDs are rejected at registration time with a clear error.

---

## Clap Features

Forge uses [clap](https://docs.rs/clap) for argument parsing. All clap features work:

### Subcommands (not recommended — use flat namespace instead)

Forge commands are flat (`import:users`, not `import users`). This is intentional — flat namespaces are easier to discover via `--help` and don't conflict with the framework's command routing.

### Value Parsers

```rust
Arg::new("count")
    .value_parser(clap::value_parser!(u64))  // parsed as u64
    .help("Number of items")

// Read as:
let count: u64 = *inv.matches().get_one::<u64>("count").unwrap();
```

### Multiple Values

```rust
Arg::new("tags")
    .long("tag")
    .action(ArgAction::Append)  // can pass multiple times
    .help("Tags to apply")

// cargo run -- my-command --tag foo --tag bar
let tags: Vec<&String> = inv.matches().get_many::<String>("tags")
    .map(|v| v.collect())
    .unwrap_or_default();
```

### Boolean Flags

```rust
Arg::new("verbose")
    .long("verbose")
    .short('v')
    .action(ArgAction::SetTrue)

let verbose = inv.matches().get_flag("verbose");
```
