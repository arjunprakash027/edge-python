FROM rust:1-bookworm

RUN cargo install cargo-afl

WORKDIR /app

COPY . .

RUN cd compiler/fuzz-afl && cargo afl build --release

CMD ["bash", "compiler/fuzz-afl/deploy.sh"]
