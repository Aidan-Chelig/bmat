set dotenv-load := true

# Open a BMAT project or material in the editor. Extra arguments are passed
# directly to edbmat (for example: `just edbmat assets/example`).
edbmat *args:
    cargo run --bin edbmat -- {{args}}

check:
    cargo check --bins
