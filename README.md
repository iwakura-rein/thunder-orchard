Hey iwakura-rein, could you please go into Settings and allow Issues on your fork?


# Thunder-Orchard

## Building

Check out the repo with `git clone`, and then

```bash
$ git submodule update --init
$ cargo build
```

## Running

```bash
# Starts the RPC-API server
$ cargo run --bin thunder_orchard_app -- --headless

# Runs the CLI, for interacting with the JSON-RPC server
$ cargo run --bin thunder_orchard_app_cli

# Runs the user interface. Includes an embedded 
# version of the JSON-RPC server. 
$ cargo run --bin thunder_orchard_app -- --headless
```
