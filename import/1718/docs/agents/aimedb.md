# Workstream: aimedb

Rules: ../../AGENTS.md. Facts: ../RESEARCH.md. Work in your own folder; the runtime is changed by one session only.

Decode the client's AiMeDB protocol (aime.naominet.jp:22345 via getaddrinfo ~VA 0xD84236; host ptr VA 0x153CE7C → string VA 0x122CE50; AES rijndael-alg-fst, key expected 'Copyright(C)SEGA'): header, commands, responses so an access code resolves to a stable aimeId. Give the seat-1 string patch to 'localhost'. Write a stdlib-only local server (pure-Python AES if needed) with a FIPS-197 known-answer --selftest.

Deliverable: FINDINGS.md (conclusions first, addresses, exact patches/configs, install steps) + code with a passing self-test; update ../TASKS.md.
