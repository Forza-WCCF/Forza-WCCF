# Workstream: catalogue

Rules: ../../AGENTS.md. Facts: ../RESEARCH.md. Work in your own folder; the runtime is changed by one session only.

Build the overlay catalogue (catalogue.tsv + cards/<number>.png) for 2017-18 from the user's game files, in the column format kit/scripts/cards.py writes. Partial result: research/catalogue/cards1718.py (card→record table int32[0x2CC1] at VA 0x015B5B48, club/nation names char*[286] at VA 0x0154C428, player_data.bin 8,434 records × 0x620, same xaf0 archive) — review, verify spot checks, then install.

Deliverable: FINDINGS.md (conclusions first, addresses, exact patches/configs, install steps) + code with a passing self-test; update ../TASKS.md.
