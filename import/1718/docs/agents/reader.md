# Workstream: reader

Rules: ../../AGENTS.md. Facts: ../RESEARCH.md. Work in your own folder; the runtime is changed by one session only.

Find why seat 1 picks OFFLINE guest play (Aime screen object field +0x6C, Scene_ADV_Aime_GuestPlay_Offline) and the minimal config/patch to go online locally; find the Aime NFC reader device (aimelib 3.07.7, cAiMeNFCRW; COM2?) and its wire protocol; write a named-pipe stand-in (style of kit/scripts/_icc_reader.py) presenting one fixed Aime card, wired via mxhook MXHOOK_COM, with an offline --selftest.

Deliverable: FINDINGS.md (conclusions first, addresses, exact patches/configs, install steps) + code with a passing self-test; update ../TASKS.md.
