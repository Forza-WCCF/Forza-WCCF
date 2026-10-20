# One image for every role of the NAT harness (README.md): the binaries come from the host's target/release, mounted.
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends iproute2 iptables && rm -rf /var/lib/apt/lists/*
