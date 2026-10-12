# Workstream: gamesvr

Rules: ../../AGENTS.md. Facts: ../RESEARCH.md. Work in your own folder; the runtime is changed by one session only.

Document request/response layouts for COACH_LIST 271/272, SAVE_DATA 275/276, SAVE_DATA_SET 277/278 (which bytes are the save blob, checksums), COACH_LIST_ADD 279/280, EXTENDS_*, INJUSTICE_COUNT, AIME_UNLOCK, PASS_SET, ITEM_LIST, OLD_*; how a new aimeId with no clubs is answered; minimal healthy answers for the controller's startup messages. Extend kit/scripts/_gamesvr.py to persist clubs per aimeId/coachId (atomic writes + backup) with a round-trip --selftest. Control is a relay: the client builds/parses these.

Deliverable: FINDINGS.md (conclusions first, addresses, exact patches/configs, install steps) + code with a passing self-test; update ../TASKS.md.
