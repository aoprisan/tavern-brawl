set dotenv-load

default: run

# live: referee is Jev, key from .env (TYPESAFE_API_KEY)
run:
    cargo run --release --bin tavern-brawl

# offline rehearsal: keyword mock instead of Jev
mock:
    MOCK=1 cargo run --release --bin tavern-brawl

# colleagues join from their phones on the LAN
lan:
    HOST=0.0.0.0 cargo run --release --bin tavern-brawl

check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test

# one raw SDK call to verify the key
ping:
    cargo run --release --bin ping
