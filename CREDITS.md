# Credits

## People

- **[flack0x](https://github.com/flack0x)** started the 2010–11 project and wrote most of its original code. Their
  kits 1 to 6 (`WCCF-Kit-1011`) first ran the original server, the projector and a player cabinet on one PC, with:
  - the setup and the run scripts (`SETUP.bat`/`PLAY.bat`, then `setup.py` and `play.py`), and the hook that stands in
    for the cabinet's hardware, later shipped as `wccfk.dll` with the game's import renamed to it so antivirus
    software leaves it alone;
  - the on-screen panel: cabinet buttons, the formation board, the card catalogue with its club and country filters,
    KEYS with game controllers, SETTINGS, and CLUB CARD with its backups and several clubs;
  - the English game text;
  - online play on a shared server: the seat desk, the match relay, the log inbox, the ping meter, and the guards
    that keep the server running through the original control program's message-handling crash;
  - the club card and contract-end research ([docs/research/CLUB-CARD-2010-11.md](docs/research/CLUB-CARD-2010-11.md),
    [docs/research/CONTRACT-END-2010-11.md](docs/research/CONTRACT-END-2010-11.md)) and the game's own manager
    transfer to a new card;
  - Kit 6's reliability work: a card is never written in place, seat leases survive a break, crash snapshots and
    automatic reports;
  - free play, SEND LOGS, the Kit 6 folder layout, the Kit 5 to Kit 6 releases and the first project front page.
- **[peakxl](https://github.com/peakxl)** maintains Forza-WCCF. In `WCCF-Kit-1011` they wrote the Rust `PLAY.exe` and
  `SETUP.exe` with updates, CI, CLEAR BAD ENDINGS, the catalogue's search words and card roles, a player card after
  each match, the players' full names in English, and the Italian game text. They wrote `WCCF-Kit-1718` (the 2017–18
  edition, built on the 2010–11 kit) and `WCCF-Installer`, and the work in this repository since the merge.

## Where this repository comes from

Forza-WCCF merges three repositories. Its history before 0.2.0 was imported from them, and commit authors were
unified in the process, so this file is the record of who did what.

| Repository | Imported at | What |
|---|---|---|
| [Forza-WCCF/WCCF-Kit-1011](https://github.com/Forza-WCCF/WCCF-Kit-1011) | `b40b84a` (Kit 6) | the 2010–11 kit; this repository started as its fork |
| Forza-WCCF/WCCF-Kit-1718 | `a7249b9` | the 2017–18 kit |
| Forza-WCCF/WCCF-Installer | `073d719` | the installer (`crates/installer`) |

A few commits in `WCCF-Kit-1011` (the Kit 5.4 release workflow) were written with an AI coding assistant.

## Third-party software and fonts

- **FreeType** 2.13.3 (`vendor/freetype-sys/freetype2`), under the FreeType License (FTL); the `freetype-sys`
  bindings are MIT. See `vendor/freetype-sys/LICENSE` and `freetype2/LICENSE.TXT`.
- **Fonts** in `assets/fonts/`, shipped in both ZIPs: Hack (MIT, with the Bitstream Vera license;
  `Hack-LICENSE.txt`), Barlow Condensed and Noto Sans JP (SIL Open Font License 1.1; `*-OFL.txt`).
- **Rust crates**: each under its own license (see `Cargo.lock` for the full set); the installer's list is
  [crates/installer/THIRD-PARTY-LICENSES.json](crates/installer/THIRD-PARTY-LICENSES.json), which `cargo xtask check`
  compares with `cargo tree`.
- **Python** 3.13 embeddable and Pillow 11.3 (`legacy/python`): PSF License (`legacy/python/LICENSE.txt`) and Pillow's
  MIT-CMU (HPND) license. It stays in the tree while the legacy Python scripts and their tests are kept; the ZIPs do not
  ship it.

World Club Champion Football and its files belong to the publisher. Forza-WCCF holds none of the original files and is
not affiliated with the publisher.
