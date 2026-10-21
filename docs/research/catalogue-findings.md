# 2017-18 card catalogue: findings (2026-10-08)

## Conclusions

- `cards1718.py` reads the 2017-18 data correctly. The card number to record mapping, names, line (GK/DF/MF/FW),
  club, nationality, birth date, height, weight, the six stats and the pictures agree with the cards' own printed
  backs for all 13 spot-checked cards. For the 3,927 card numbers 2010-11 shares, names, line, birth date and stats
  also agree with the 2010-11 catalogue. The one exception is card 2854, which the publisher reused (2010-11 Bernd SCHUSTER,
  2017-18 Thomas MÜLLER). Club, nationality and season labels differ for about 190 shared cards; the 2010-11 ones
  were inferred, and the 2017-18 ones come from the exe's name table and VerName.txt.
- The catalogue holds 8,434 rows: **8,396 real cards and 38 placeholders**. Placeholders have `is_placeholder` = 1,
  and the overlay skips them. 36 are edition 13 "選手以外" (not players): 11 "Unknown" stand-ins, the chairperson,
  3 referees, 3 coaches, 11 GUESTPLAY default players, the navigator Yurie OSAWA and 6 network coaches. The other 2
  are the joke player Diego TERADONA of the publisher's own club (3509 and 9951). Card 3509's picture is a scrawled "ダミー" (dummy).
  Calling 9951 a placeholder too is a judgement (guess): it is a staff Maradona parody that the 2010-11 catalogue
  offered.
- The overlay up to 2026-10-08 showed at most 4,096 cards (`load_catalogue`: `cap = 4096`; `g_res[4096]`); the
  catalogue therefore lists rows newest season first. With that overlay, players get 2012-13 through 2017-18
  (3,879 cards) and 217 of the 2011-12 cards. The cap is now 16,384 in `kit/source/overlay/wccfpanel.c`.
- The self-check passes: `python cards1718.py GAME` runs 33 checks and writes nothing. It exits 1 on any
  regression; a wrong club name fails it, which was tested. It needs Pillow (the kit's Windows Python has it).

## What was wrong and what changed

1. Card 3509 (TERADONA, dummy picture) was offered as a real ALL TIME LEGEND FW with a total of 100. 3509 and
   9951 are now placeholders (`NOT_PLAYERS`).
2. Row order: card-number order would have filled the overlay's 4,096 slots with 2001-02 to ~2009-10 cards and
   dropped every 2016-17 and 2017-18 card. The order is now season descending; the sort is stable, so cards stay in
   card-number order within a season.
3. The spot checks were 2 hard-coded cards. They are now a `KNOWN` table of 13 cards (all four lines, editions
   2013-14 / 2016-17 / 2017-18) plus checks on placeholders and on the ATLE stand-in stats.
4. The self-check mode (GAME only) is new. The script now finds `cards.py` and `ys_lzw.py` in `../../kit/scripts`
   by itself, so it ran in place.

Nothing else needed fixing. The previous output's placeholder rows (ERROR_CARD.dds, club id 0 "アタランタBC")
already had `is_placeholder` = 1 and blank club and nationality names, so the overlay never showed them.

## Addresses and formats (client_Release.exe, SDDY)

- Card table: `int32[0x2CC1]` at VA 0x015B5B48 maps card number to record index (-1 = none). The code at VA
  0x453C0B does `cmp ecx,0x2CC0 / ja / mov eax,[ecx*4+0x15B5B48]`, and the table address appears as an immediate in
  about 180 places. The file holds the table already filled: it maps 8,434 numbers onto records 0..8433, one each,
  and record +0x288 equals the card number for every record. Card numbers run 1..11456.
- Club/nationality names: `char*[286]` at VA 0x0154C428 (clubs 0-189, then nationalities 0-95, cp932). No code
  references the table's start directly; it was found by content. The mapping is confirmed by the cards
  (33 FC Barcelona, 39 PSG, 28 Liverpool, 101 Dortmund, 43 Bayern ...).
- `prog_data/player_data/player_data.bin`: 8,434 records × 0x620. The 2010-11 fields sit at +0x288 (card_no u16
  +0x288, set_no +0x28A, club u16 +0x28C, edition +0x29E, rarity +0x29F, card code +0x2A0, names +0x2B0.., line
  +0x3B9, birth +0x3BA, height/weight +0x3CA/B, nationality +0x3CC, stats +0x3D7..0x3DC, total +0x3DD, image name
  +0x564). set_no is the printed collector number (7049 MESSI: set_no 240, back "240/272").
- `prog_data/player_data/VerName.txt`: edition labels; the line index, counted from 0, is the edition byte.
- `data/wccf_data.xaf` (xaf0) holds 14,766 `data/player_card/` files. Each card's `<image>.dds` is 128×128 (the
  card face in the left ~90 columns, then edge smear). 3,039 cards (2013-14 onward) also carry `_F`/`_B` 256×512
  front and back pictures, and the backs print name, club, shirt number, birth date, height, weight, nationality,
  line and the six stats; they were the source for the spot checks.
- Rarity byte: 1 REGULAR 4,385 · 2 SPECIAL 2,455 · 3 RARE 1,260 · 4 LEGEND 60 · 5 ALL TIME LEGEND 236 (real cards).

## Spot checks (catalogue = card back, every field)

| card | edition | name | line | club | nat. | OFF DEF TEC POW SPD STA | born | cm/kg | source |
|---|---|---|---|---|---|---|---|---|---|
| 7049 | 2017-18 | MESSI | FW | FC Barcelona | Argentina | 20 8 20 16 19 15 | 1987-06-24 | 170/72 | back 1718_9320_B |
| 7064 | 2017-18 | C. RONALDO | FW | Real Madrid | Portugal | 20 5 19 19 19 15 | 1985-02-05 | 185/80 | back 9335_B |
| 6840 | 2017-18 | M.SALAH | FW | Liverpool | Egypt | 16 8 17 14 19 15 | 1992-06-15 | 175/71 | back 9111_B |
| 6904 | 2017-18 | LEWANDOWSKI | FW | Bayern | Poland | 19 6 17 18 16 16 | 1988-08-21 | 185/79 | back 9175_B |
| 7061 | 2017-18 | MODRIĆ | MF | Real Madrid | Croatia | 16 12 19 13 17 17 | 1985-09-09 | 174/65 | back 9332_B |
| 6883 | 2017-18 | KAGAWA | MF | Dortmund | Japan | 16 10 17 13 16 16 | 1989-03-17 | 175/68 | back 9154_B |
| 7054 | 2017-18 | SERGIO RAMOS | DF | Real Madrid | Spain | 12 19 14 18 15 17 | 1986-03-30 | 183/75 | back 9325_B |
| 7127 | 2017-18 RARE | NEUER | GK | Bayern | Germany | 10 20 14 18 14 13 | 1986-03-27 | 193/92 | back 9398_B |
| 6954 | 2017-18 | BUFFON | GK | Juventus | Italy | 8 20 13 18 13 13 | 1978-01-28 | 191/91 | back 9225_B |
| 5980 | 2016-17 | BUFFON | GK | Juventus | Italy | 8 20 13 18 13 13 | 1978-01-28 | 192/94 | back 1617_8257_B |
| 6826 | 2017-18 | MIGNOLET | GK | Liverpool | Belgium | 7 18 12 18 12 11 | 1988-03-06 | 193/87 | back 9097_B |
| 1007 | 2013-14 | BA | FW | Chelsea | Senegal | 19 5 15 18 17 14 | 1985-05-25 | 189/85 | back (2026-10-07) |
| 4101 | 2013-14 | SZCZESNY | GK | Arsenal | Poland | 7 18 12 18 12 11 | 1990-04-18 | 195/93 | back (2026-10-07) |

The front pictures (`cards/<no>.png`) of all of the above, plus 7130, 6873, 7091, 3312, 7143, 6402, 4659, 7131 and
443, show the right player in the right kit. Card 7143 MARADONA (ATLE) and 7131 SAMPAIO (ATLE-RE) print "SECRET"
and "-" for stats on their backs; their records hold 16 16 16 16 16 20.

## Install

This script is retired: Launcher.exe's Setup now generates the catalogue into the overlay folder of the runtime (about 2 minutes).

This writes `catalogue.tsv` and `cards\<no>.png` (8,434 PNGs of 128×128, about 120 MB) next to `wccfpanel.dll`,
where the overlay reads them. The board thread loads the catalogue once at start, so it appears after the next
seat-1 start; `seat1\wccfpanel.log` then says `browser: catalogue 8396 cards` (4096 with an overlay built before
the cap was raised). `kit/overlay/catalogue.tsv` and `kit/overlay/cards/` are in `.gitignore` and must stay out of
git (derived from the publisher's).

## Open issues

- **`kit/scripts/setup.py` `setup_cards`** still calls the 2010-11 `cards.main` and compares against
  `cards.RECORDS` (3,927). On 2017-18 files it would fail the record-count check; the 2017-18 setup path does not
  call it (`tools/setup1718.py`), so the catalogue is generated by the command above.
- **ATLE/legend stats**: 192 legend cards (ATLE, HOLE, ATLE-RE, LEOC, SATLE, KOLE, LEU, KLE; 175 with rarity 5 and
  17 ATLE-RE with rarity 1) store 16 16 16 16 16 20 = 100. Their backs say "SECRET". The overlay shows and sorts
  them as total 100. What the match engine uses for them is unknown; one guess is the unparsed `hidden_4df_523`
  bytes.
- **Old cards in a 2017-18 match**: the game's table maps all 8,434 records, but nobody has shown yet that the
  client accepts every edition on the card table. That is the board workstream's job.
