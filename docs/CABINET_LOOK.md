# Cabinet look: reference for the egui redesign (roadmap step 9.2)

Inspiration only: this file holds words, measurements and estimated colour values. It carries no logos, trademarks or original pictures. The publisher is not named; "original" means the game's own files and text.

**Evidence level, read first.** Tags: **[S]** sourced (link in section 9), **[L]** from local project notes, **[I]** inference or estimate. Online coverage of the cabinets is thin: no source found gives monitor sizes, a touch-screen specification, button colours, fonts or sound descriptions, and no cabinet photo could be viewed in this research pass. Every colour and typography value below is therefore **[I]**, an estimate from the cabinet's known role (a green pitch, a dark satellite monitor) and general arcade-sports conventions, and must be checked against photos or a screenshot of the running game before it is frozen. The brief assumed a touch screen; no source confirms one on the 2010–11 or 2017–18 satellites (see section 2).

## 1. Summary: the feel in seven actions

1. **The pitch is the interface.** The original is not played with a joystick: the player places physical cards on a field-shaped surface, and the card positions are the formation [S1, S2]. Make the FORMATION BOARD the hero of the side panel: a pitch with three zones (defence, centre, attack) and cards that sit where you drop them [S3].
2. **Position comes from the zone, not the label.** A card's role follows the zone it sits in, not its printed position [S3]. Show the zone bands on the board and re-label a card live as it crosses a band edge.
3. **Cards are physical objects.** Players hold real cards, 81 mm x 58 mm, landscape-short of a bank card [S2]. Draw cards as upright rectangles with a 81:58 (about 1.40:1) proportion, a thin bright rim and a face/back split; give them a small lift (shadow, 2 px raise) when dragged and a settle bounce when dropped [I].
4. **A small, fixed button vocabulary.** The cabinet has five tactic buttons, a data button, a key-player button, a shoot button and a goalkeeper button [S3, S4]. Tactic buttons light up when active [S3]. Render these as chunky, lit, press-down buttons, not flat text buttons.
5. **Sports-broadcast confidence.** The main screen is a TV-style match view with a score bar and radar-style data display toggled by the data button [S2, S3, S5]. Use a dark neutral ground, one saturated pitch green, and bright accents for state [I].
6. **Short loops.** Team management takes about 2 minutes, a match about 5 minutes [S2]. Screens should read at a glance: few controls, big targets, no dense forms.
7. **Mixed Latin/Japanese type, heavy and condensed for numbers.** Stat numbers and labels are large and bold; Japanese labels sit beside Latin ones [I]. Ship the three first-class languages (EN, IT, JA) with one font family that has both scripts.

## 2. Cabinet and card table

**Shape [S].** The satellite is a seat-and-table unit: "satellite seats: 8 (4 in the 4-satellite version) plus a main monitor", ALL.Net compatible [S3, S4]. The top of the satellite is a field-shaped surface where the player cards are placed; card placement sets the formation and cards can be moved during a match [S1]. The main cabinet (8P and 4P variants, later a MINI) uses DLP projectors for the shared main picture [S1]. The original system started as ten NAOMI 2 boards behind a server PC with nine monitors for eight players [S2]. The 2010–11 and 2017–18 editions run on the RingEdge board family [S2, L].

**Card table [S/L].**
- Surface is a flat card reader developed in-house, chosen over a deck reader [S2]. The 2017–18 client names it "Flat Reader 3 (COM%d)" and has an emulator mode [L: `ringedge/WCCF-Kit-1718/docs/TASKS.md:43`, `ringedge/1718-kit-20261007/HANDOFF.md:53`].
- Card size 81 mm x 58 mm (the ISO ID-1 bank card is 85.60 x 53.98 mm) [S1].
- The operation panel splits into three zones: defensive zone near your own goal, central zone, attacking zone near the opponent's goal [S3].
- A team has 11 cards on the field and a squad of up to 16 [S1, S6].
- Forward and backward card gestures switch offence/defence, and the reader detects substitutions [S2].
- One card is dispensed per credit (the original paid 300 yen per play, 500 yen for two, in 2009–11; 200 yen per credit later) [S1, S3, S5].

**Touch screen.** No source found describes a touch screen on these satellites. The same menu buttons that drive play also navigate menus [S2]. Treat a touch menu as **unconfirmed [I]**. Designers should assume button-and-card input, which suits a mouse-first desktop UI anyway.

**Differences in the table between editions.** 2010–11 starts a session with an inserted IC club card [L: `docs/research/CLUB-CARD-2010-11.md`] and shows a club-card insert prompt on the attract screen [L: `docs/research/FREE-PLAY-2010-11.md:172`]. From 2012–13 only Aime cards are accepted; 2017–18 sessions start from an Aime or QR screen with red on-screen notices ("二次元コード!", "Aimeカード!") [S1, L: `docs/2017-18/RESEARCH.md:52`]. Rare cards from 2013–14 carry a QR serial [S1].

## 3. Screens and menus

The sources describe the screens only in outline; visual detail is **[I]** unless noted.

- **Attract/title.** Credit and free-play state are drawn on the attract screen; inserting the card starts play [L: `docs/research/FREE-PLAY-2010-11.md:113,172`]. Design: calm loop, one clear "insert card" call to action.
- **Club/team management.** Cards are placed on the table; the screen shows the lineup and fit. A player played outside their position loses ability, and fit is shown on screen and on the card (this is stated for the successor game, whose core play matches the original) [S6]. Translate into the FORMATION BOARD: out-of-position cards get a visible warning tint.
- **Main monitor.** Shows tournament results and the match [S5]. The data button toggles on-screen stats [S1, S3]; direction keys show the manager's current instruction on screen [S1].
- **Contract end / game over.** Results list Prize Money, Previous salary and Last Pay [L: `docs/research/CONTRACT-END-2010-11.md:94`]. Staff screens (test menu, Club Card Recovery) are plain, text-first screens [L].
- **Mode choice.** In-store versus nationwide network battle appeared in the 2009–10 edition [S5].
- **Card back, as a stat-layout model.** Card backs show main and sub position, ability values, consistency and injury resistance [S6]. Use this order for the CATALOGUE stats pane.

## 4. Buttons

Sourced list for the 2009–11 satellite [S3, S4]: card movement (not a button), **five tactic buttons**, **data display**, **key player**, **shoot**, **goalkeeper**. The 2012-era list in the Japanese encyclopedia adds behaviour: shoot is manual (shots are not automatic), the goalkeeper button is held to send the keeper forward, the key-player button is held while morale is at maximum to fire a special order [S1]. The tactic cluster is four directions plus a centre button; orders are left, right, centre, counterattack and pressure, and several can be pressed together [S2, S3 describe the same set]. Lit tactic buttons narrow pass options when the side directions stay lit [S3].

Photo findings (cabinet from 2006–07, unchanged to 2017–18; owner decision 2026-10-16, one look for both editions). Left plate: a diamond of four yellow triangles (up CENTRAL, down COUNTER, left L-SIDE, right R-SIDE) round a red PRESS square at the centre, with a small green DATA square beside it; the labels are printed beside the triangles. Right plate: a big red SHOOT and a big blue KEEPER round button side by side, a small yellow round KEY PLAYER and a small green round START. The game's own legend reads green start, yellow KEY, red decide, blue back.

Design translation **[I]**:
- The panel draws exactly that layout (`deck_layout` in `crates/forza/src/ui/panel.rs`): shapes are round, square or triangle, and clicks count only on the shape.
- Small buttons carry their label beside them, the big ones inside.
- Buttons keep an idle, hover and pressed state (3 px travel). Key presses do not light them; only an active tactic order lights its triangle or square.
- The MACHINE group (CARD only: free play needs no COIN, and the BACK button sent nothing), the formation board and the bottom bar stay modern.
- KEYS panel: show the physical layout of the cabinet cluster next to each bound key, so the binding reads as "this key presses that cabinet button".

### Window layout (owner decision 2026-10-17: model on a 2K monitor)

The game renders 1440x900 in both editions (the panel's swap chain follows the game window's client area; `fakegame` asserts it). The panel window maximizes once after the first frame, so a 2560x1440 monitor gives a client of about 2560x1369 (title bar, taskbar) and no window sizing code is needed. `forza::ui::panel::layout` is a pure function of the window rect, tested for no overlaps, 16:10 and the old size at 1440x900:

- Scale: `scale(w, h)` is 1 point a pixel from 1440x900 up (a bigger window is a bigger sheet), 2 from 2880x1800, and the ratio to 1440x900 below that, so the sheet is never smaller than 1440x900.
- The picture takes the largest 16:10 size that leaves the board 580 points and the deck 300 plus the 60-point bar. At 2560x1440 that is 1728x1080, exactly 1.2x the game, the board gets 832 and the deck stays 300 high. A maximized 2560x1369 window gives about 1614x1009 (1.12x), board 920. At 1440x900 nothing changes (860x537.5, board 580). At 1920x1080: 1102x689 or larger.
- The board takes what the picture leaves, capped by what its own height uses (pitch 1.25 x width plus bench and hint); the picture is centred in the middle. The deck keeps `deck_layout` and is centred under the picture.
- Not snapped to an integer scale: 1.2x needs 1440 points of height, which a maximized window lacks; the picture is resampled smoothly instead.

### The projector window (owner decision 2026-10-21: the in-panel view is gone)

The projector (the second game process, 1600x600: two 800x600 screens side by side) has its own window again, in both editions (`crates/panel/src/projector.rs`, `fullscreen.rs`):

- The window gets a caption and a sizing border and starts at the game's 1600x600 client, scaled down (shape kept) and moved inside the monitor's work area when the screen is smaller. The game's back buffer keeps its size (no device reset).
- At the game's own client size the game's Present runs untouched. At any other size the panel draws the frame letterboxed on black bars on a swap chain of its own at the client size (one StretchRect) and shows that instead; any failure falls back to the game's Present.
- F11 or a double-click: borderless full screen on the monitor the window is on. Esc leaves it. The panel window's F11 uses the same helper (`fullscreen::toggle`: style and placement saved, WS_POPUP over the monitor, both restored on leaving); Esc there also leaves and still reaches the panel.
- Gone: the PROJECTOR switch, the frame section shared with seat 1, the move off-screen and `WCCFPANEL_PROJECTOR_WINDOW`.

## 5. Colour palette (all values estimates [I])

No source gives hex values. The values below come from the cabinet's role and arcade-sport conventions and are starting points for the theme. Tune by sampling a screenshot of the running game, which the project can take locally (window capture only, no commit of the pictures).

| Name | Role | Hex (estimate) |
|---|---|---|
| Night ground | Window background, panel base | `#0B1220` |
| Deep panel | Cards, frames, inactive wells | `#142033` |
| Panel rim | Hairlines, card outline | `#2B3E5E` |
| Pitch green | Formation board field | `#1E7A3C` |
| Pitch stripe | Alternate mowing stripe | `#228A43` |
| Pitch line | Field markings | `#E8F2E8` |
| Lit amber | Active tactic button, selection | `#FFB400` |
| Shoot red | Shoot button, danger, errors | `#D93025` |
| Keeper blue | Goalkeeper, links, focus ring | `#2F7DE1` |
| Data cyan | Stat bars, data display | `#2CC6D9` |
| Gold rarity | Rare/top cards, highlights | `#D4A72C` |
| Text white | Primary text | `#F2F5FA` |
| Text dim | Secondary text, labels | `#8FA0BA` |
| Position GK / DF / MF / FW | Position chips | `#E0A800` / `#2F7DE1` / `#2FB36B` / `#D93025` |

Contrast check: Text white on Night ground is far above 7:1; Lit amber text on Night ground is above 4.5:1. Position colours follow the common football convention (keepers warm yellow, defenders blue, midfielders green, forwards red) [I].

## 6. Typography [I]

No source names fonts. The original screens use large bold sans type with heavy numerals and Japanese captions; the project already plans open fonts shipped with an OFL file (`assets/fonts/`) [L: `docs/ROADMAP.md:159`].

- **Latin:** a bold, slightly condensed sans for headings and stat numbers (weights 700 and 800); a regular 400 and medium 500 for body. Candidates under OFL: Barlow / Barlow Condensed, Rajdhani, or Oswald for numerals.
- **Japanese:** Noto Sans JP at 500 and 700, paired with the Latin face; set kana/kanji about 0.95x the Latin size so lines align.
- **Sizes for the egui text styles:** Heading 20 px bold, Body 14 px, Button 14 px bold, Small 11 px, Monospace 13 px; stat numerals 22 px in the condensed face with tabular figures.
- **Case:** panel names in capitals (FORMATION BOARD, CATALOGUE, KEYS, SETTINGS, CLUB CARD), matching the project's own naming [L: `docs/ROADMAP.md:597`].
- EN, IT and JA all ship (AGENTS.md section 4): test that Italian labels (longer than English) and Japanese labels (no spaces) fit the same button widths.

## 7. Motion and sound

**Sourced facts.** Tactic buttons light up [S3]; the successor cabinet flashes an LED lamp on a goal along with the commentary [S6]. Hot lines between linked players are drawn on screen in the successor [S6]. The original has live commentary and match audio; no source describes the menu sounds.

**Proposals [I].**
- Motion budget: 120 to 160 ms for hover and press, 200 to 250 ms for card drop-settle, no motion longer than 300 ms. Set `animation_time` to about 0.15 s.
- Card drag: lift with a shadow and 3 degrees of tilt; zone bands on the pitch brighten as the card enters them; the card snaps to the nearest slot on drop.
- Lit tactic buttons pulse once on activation, then stay steady.
- Sounds, if the project adds any: a short click for buttons and a soft card-on-surface tap for drops; none should loop. Keep the sounds off by default in the launcher and the installer, on in the in-game panel.

## 8. 2010–11 versus 2017–18

| Aspect | 2010–11 | 2017–18 |
|---|---|---|
| Release | Autumn 2011 in arcades (the 2009–10 version came in December 2010) [S2, S5] | January 2018 [S2] |
| Board | RingEdge, projector main display [S1, S2, L] | RingEdge, projector plus match engines [L] |
| Session start | IC club card inserted [L] | Aime (or compatible IC) card, QR serial for rare cards; other IC cards accepted from 2018 [S1] |
| Card table | Earlier flat reader | "Flat Reader 3" [L] |
| Card art | Cards redesigned in the 2009–10 refresh [S5] | Rarity shown through QR-registered rare cards [S1] |
| Look | Older, flatter UI, fewer effects [I] | Newer, glossier 3D presentation [I] |
| Stat/player data | Position, abilities, condition, fatigue, mastery, injury per player [L: `docs/research/CLUB-CARD-2010-11.md`] | Same family of stats, larger roster [L] |

Practical effect for the theme: one design language with two accent variants is enough. Keep the same layout; the 2010–11 variant can be slightly cooler and flatter, the 2017–18 variant slightly warmer with a gold rarity accent **[I]**. The successor cabinet (2019) moved to a larger enclosed cabinet with LED lighting, five buttons A to E and on-demand printed cards; it is not a target but shows the direction the look took [S6, S7].

## 9. What translates to a mouse-first desktop UI, and what does not

**Translates.**
- The pitch with three zones and drag-and-drop cards: `Sense::drag` on card widgets, zone bands drawn behind them [S3].
- The fixed button vocabulary as a docked strip: five tactic buttons, data, key player, shoot, goalkeeper, each with a keyboard binding shown in KEYS.
- Stat bars in the card-back order (positions, abilities, consistency, injury resistance) [S6].
- Short, glanceable screens and large targets [S2].
- Lit/active button states, position chips, and the dark-ground-with-green-pitch palette **[I]**.

**Does not translate.**
- Physical card placement and card gestures (forward/backward to switch offence/defence, rubbing cards for hot lines): replace with drag, a toggle and a click sequence [S2, S6].
- Hold-to-trigger buttons (goalkeeper, key player): a mouse can hold, but offer a toggle too.
- Several buttons pressed at once (combined tactic orders): map to modifier keys or multi-select toggles.
- Cabinet lighting (LED goal flash) and the shared main-screen projector: skip, or reduce to a brief border flash.
- Credit/coin and card-insert prompts: not needed in a free-play kit.
- Pre-printed card artwork: the project must not copy original pictures; draw neutral card frames and use text for the face.

## 10. Sources

Online:
- **[S1]** Japanese encyclopedia entry on the game (cabinet types, field-shaped top, card size 81 x 58 mm, Aime/QR rules, button descriptions, pricing, projectors): https://ja.wikipedia.org/wiki/WORLD_CLUB_Champion_Football
- **[S2]** English encyclopedia entry (development, flat card reader, controls, version dates, board generations): https://en.wikipedia.org/wiki/World_Club_Champion_Football
- **[S3]** Game Watch "Pickup Arcade" feature (satellite seats, five tactic buttons, three-zone operation panel, lit buttons): https://game.watch.impress.co.jp/docs/series/pickupac/522721.html
- **[S4]** Game Watch 2008 launch article (same control list, card movement, ALL.Net): https://game.watch.impress.co.jp/docs/20081121/wccf.htm
- **[S5]** Game Watch news on the 2009–10 edition (in-store vs network battle, one card per play, price): https://game.watch.impress.co.jp/docs/news/411110.html
- **[S6]** Game Watch report on the 2019 successor (enlarged cabinet, flat reader, card-back layout, buttons A to E, LED): https://game.watch.impress.co.jp/docs/news/1136586.html
- **[S7]** 4Gamer report on the 2019 successor (cabinet fully redesigned, on-demand printed cards): https://www.4gamer.net/games/426/G042669/20180927051/
- Searched without usable detail: 4Gamer title page https://www.4gamer.net/games/114/G011427/ (version dates and prices only); an arcade hobbyist forum thread on a satellite-terminal WCCF setup (card reader model HW210, networked satellites; page blocked to fetching, seen only in search snippets): https://www.arcade-projects.com/threads/naomi-2-satellite-terminal-setup.28745

Local:
- `docs/ROADMAP.md` (step 9, lines 24, 159, 587-631)
- `docs/2017-18/RESEARCH.md` (line 52 on-screen notices, lines 79-91)
- `docs/research/FREE-PLAY-2010-11.md`, `CLUB-CARD-2010-11.md`, `CONTRACT-END-2010-11.md`
- `../ringedge/WCCF-Kit-1718/docs/TASKS.md` (line 43), `../ringedge/1718-kit-20261007/HANDOFF.md` (line 53)

## 11. Gaps to close before freezing the theme

1. Colours, fonts and screen resolution: sample a window capture of the running game (PrintWindow, game window only) and replace the estimates in section 5.
2. Touch screen: confirm from a photo or a cabinet manual whether the satellite has one.
3. Monitor size and satellite dimensions: no source found.
4. Sounds: record or list the original's menu cues from the runtime's own audio files, locally, without committing them.
