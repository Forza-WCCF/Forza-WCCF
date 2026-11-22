//! The retired `test_pictures1718.py` and `test_english_pictures.py` on a scratch archive (no game).
use super::*;

fn font_file() -> PathBuf {
    super::super::tests::manifest_dir()
        .join("../../assets/fonts")
        .join(FONT)
}

#[test]
fn freetype_draws_as_pillow_11_3() {
    // Pillow 11.3 (FreeType 2.13.3, basic layout) drew this row of "D" at 72 px as 231 208 208 208 226; the
    // FreeType 2.13.2 of crates.io's freetype-sys drew 229 204 204 204 222 (vendor/freetype-sys)
    let font = Font {
        lib: freetype::Library::init().unwrap(),
        data: std::rc::Rc::new(fs::read(font_file()).unwrap()),
    };
    let (w, h, m) = font.line(72, "D").unwrap().ink().unwrap();
    assert_eq!((w, h), (29, 51));
    assert_eq!(&m[8 * w + 9..8 * w + 15], &[255, 231, 208, 208, 208, 226]);
    assert_eq!(font.line(100, "Hgjy").unwrap().height().unwrap(), 94); // Pillow: getbbox -73..21
}

#[test]
fn ys_pack_round_trips_across_a_table_reset() {
    let mut d: Vec<u8> = (0..40000u32)
        .map(|i| ((i * 7 + (i >> 5)) & 255) as u8)
        .collect();
    d.extend(b"WCCF".repeat(3000));
    assert_eq!(lzw(&ys_pack(&d), d.len()).unwrap(), d);
    assert_eq!(lzw(&ys_pack(b"a"), 1).unwrap(), b"a");
}

fn dds_header(w: u32, h: u32, mips: u32, fourcc: &[u8; 4], bits: u32, masks: [u32; 4]) -> Vec<u8> {
    let pf = if fourcc != &[0; 4] {
        4
    } else if masks[3] != 0 {
        0x41
    } else {
        0x40
    };
    let mut d = b"DDS ".to_vec();
    for v in [
        124,
        0x1007 | if mips > 1 { 0x20000 } else { 0 },
        h,
        w,
        0,
        0,
        mips,
    ] {
        d.extend(u32::to_le_bytes(v));
    }
    d.resize(76, 0);
    d.extend(32u32.to_le_bytes());
    d.extend(u32::to_le_bytes(pf));
    d.extend(fourcc);
    d.extend(bits.to_le_bytes());
    for m in masks {
        d.extend(m.to_le_bytes());
    }
    d.extend(0x1000u32.to_le_bytes());
    d.resize(128, 0);
    d
}

#[test]
fn uncompressed_pictures_encode_back_to_their_bytes() {
    // A8R8G8B8, 4 mips: the mips are Pillow's BOX halvings of the top level
    let top: Vec<u8> = (0..64)
        .flat_map(|i| {
            let (x, y) = (i % 8, i / 8);
            [
                (x * 32) as u8,
                (y * 32) as u8,
                (255 - x * 16) as u8,
                if (x + y) % 3 != 0 { 255 } else { 0 },
            ]
        })
        .collect();
    let mut argb = dds_header(8, 8, 4, &[0; 4], 32, [0xFF_0000, 0xFF00, 0xFF, 0xFF00_0000]);
    let (mut lv, mut w) = (top.clone(), 8);
    for n in 0..4 {
        if n > 0 {
            lv = pil::resize_box_rgba(&lv, w, w, w / 2, w / 2);
            w /= 2;
        }
        argb.extend(lv.chunks(4).flat_map(|p| [p[2], p[1], p[0], p[3]]));
    }
    let im = Img {
        w: 8,
        h: 8,
        px: top,
    };
    assert_eq!(encode(&argb, &im).unwrap(), argb);
    // A4R4G4B4: 4-bit values read and written back
    let mut a4 = dds_header(8, 4, 0, &[0; 4], 16, [0xF00, 0xF0, 0xF, 0xF000]);
    for r in (0..16u16).chain(0..16) {
        let (cr, cg, cb, ca) = (r, r * 5 % 16, 15 - r, r * 3 % 16);
        a4.extend((ca << 12 | cr << 8 | cg << 4 | cb).to_le_bytes());
    }
    let (w, h, px) = bcn::decode_dds(&a4).unwrap();
    assert_eq!(
        encode(
            &a4,
            &Img {
                w: w as usize,
                h: h as usize,
                px
            }
        )
        .unwrap(),
        a4
    );
    assert_eq!(
        pack_pixels(&[0, 136, 255, 17], 16, [0xF00, 0xF0, 0xF, 0xF000]),
        [0x8F, 0x10] // A 1, R 0, G 8, B 15
    );
}

/// Every scene text of the shipped recipes is present, ASCII and fits its original bytes, in both languages.
#[test]
fn shipped_scene_texts_fit() {
    let sheets = super::super::tests::manifest_dir().join("../../editions/2017-18");
    let mut n = 0;
    for it in [false, true] {
        for r in recipes(&sheets, it).unwrap() {
            for lab in r["labels"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|l| l.get("at").is_some())
            {
                let t = lab["text"].as_str().unwrap();
                assert!(
                    !t.is_empty()
                        && t.is_ascii()
                        && t.len() <= lab["len"].as_u64().unwrap() as usize,
                    "{t:?}"
                );
                n += 1;
            }
        }
    }
    assert!(n > 0);
    // centred in spaces, the length byte checked
    let mut d = [0u8, 6, 0xE5, 0x8B, 0x9D, 0x88, 0xE6, 0x95];
    let r = json!({"id": "t", "labels": [{"at": 1, "len": 6, "text": "Wins"}]});
    retext(&mut d, &r, &mut vec![]).unwrap();
    assert_eq!(&d, b"\0\x06 Wins ");
    assert!(
        retext(
            &mut d,
            &json!({"id": "t", "labels": [{"at": 0, "len": 6, "text": "Wins"}]}),
            &mut vec![]
        )
        .is_err()
    );
}

/// A recipe with a misspelt or unknown key, or a box that is not four whole numbers, stops the load: the
/// drawing reads a missing value as 0 and would draw at (0,0,0,0) on the player's PC.
#[test]
fn a_misspelt_recipe_key_stops_the_load() {
    let t = tempfile::tempdir().unwrap();
    for d in ["english", "italian"] {
        fs::create_dir_all(t.path().join(d)).unwrap();
    }
    let load = |pic: Value, it: Value| {
        fs::write(
            t.path().join("english/pictures.json"),
            json!({"pictures": [pic]}).to_string(),
        )
        .unwrap();
        fs::write(t.path().join("italian/pictures.json"), it.to_string()).unwrap();
        recipes(t.path(), true)
            .map(drop)
            .map_err(|e| super::super::tests::note_words("en", &e.0))
    };
    let good = json!({"id": "p", "where": "w", "sha256": "AB", "files": ["ui/p.dds"],
        "style": {"core": "auto", "halo": [0, 0, 0, 200], "halo_px": 1, "align": "left"},
        "clear": [[0, 0, 9, 9]], "reflect": {"src": [0, 0, 9, 4], "dst_y": 5, "alpha": [0.25, 0.03]},
        "labels": [{"box": [1, 1, 8, 8], "en": "OK", "clear": [0, 0, 9, 9], "size": 6}]});
    load(good.clone(), json!({"p": ["Sì"]})).unwrap();
    let scene = json!({"id": "s", "sha256": "CD", "files": ["ui/s.sbscene"],
        "labels": [{"at": 4, "len": 6, "en": "Wins"}]});
    load(scene.clone(), json!({})).unwrap();
    let clear_only = json!({"id": "c", "sha256": "EF", "files": ["ui/c.dds"],
        "style": {"fill": "rows"}, "clear": [[0, 1, 2, 3]], "labels": []});
    load(clear_only, json!({})).unwrap();
    let bad = |parent: &str, k: &str, v: Value| {
        let mut r = good.clone();
        r.pointer_mut(parent).unwrap()[k] = v;
        r
    };
    let mut misspelt = good.clone();
    let b = misspelt["labels"][0]
        .as_object_mut()
        .unwrap()
        .remove("box")
        .unwrap();
    misspelt["labels"][0]["bx"] = b;
    for (r, why) in [
        (misspelt, "bx"),
        (bad("/style", "halo_pz", 1.into()), "halo_pz"),
        (bad("", "refelct", json!({})), "refelct"),
        (bad("/reflect", "dst_x", 5.into()), "dst_x"),
        (bad("/labels/0", "box", json!([1, 1, 8])), "box"),
        (bad("/labels/0", "clear", json!([0, 0, "9", 9])), "box"),
        (bad("/style", "align", "centre".into()), "align"),
        (bad("/labels/0", "size", "6".into()), "size"),
        (bad("/reflect", "dst_y", Value::Null), "reflect"),
    ] {
        let e = load(r, json!({})).unwrap_err();
        assert!(e.contains(why) && e.contains("pictures.json: \"p\""), "{e}");
    }
    let mut no_len = scene.clone();
    no_len["labels"][0].as_object_mut().unwrap().remove("len");
    assert!(load(no_len, json!({})).unwrap_err().contains("at and len"));
    let e = load(good, json!({"q": ["Sì"]})).unwrap_err();
    assert!(e.contains("q is no recipe's id"), "{e}");
}

/// Every shipped recipe is well formed in both languages, and each letter drawn into a picture has a glyph in
/// the font (a missing one draws glyph 0, a box, with no warning).
#[test]
fn shipped_recipes_are_well_formed_and_every_letter_has_a_glyph() {
    let sheets = super::super::tests::manifest_dir().join("../../editions/2017-18");
    let lib = freetype::Library::init().unwrap();
    let face = lib
        .new_memory_face(std::rc::Rc::new(fs::read(font_file()).unwrap()), 0)
        .unwrap();
    let mut n = 0;
    for it in [false, true] {
        // recipes() runs check_recipe before it adds each label's "text"
        for r in recipes(&sheets, it).unwrap() {
            for lab in r["labels"].as_array().unwrap() {
                if lab.get("at").is_some() {
                    continue; // a scene's text: bytes, not drawn by us
                }
                for ch in lab["text"].as_str().unwrap().chars() {
                    assert!(
                        face.get_char_index(ch as usize).is_some(),
                        "{}: {ch:?} has no glyph in {FONT}",
                        r["id"]
                    );
                    n += 1;
                }
            }
        }
    }
    assert!(n > 0);
}

/// An area to blend rows or columns over, or a reflection, outside the picture is an error, not a panic.
#[test]
fn areas_outside_the_picture_are_errors() {
    let mut im = Img {
        w: 4,
        h: 4,
        px: vec![255; 64],
    };
    clear(&mut im, (0, 1, 3, 2), &"rows".into()).unwrap();
    clear(&mut im, (0, 0, 9, 9), &"clear".into()).unwrap(); // a plain fill is clipped
    assert!(clear(&mut im, (0, 1, 4, 2), &"rows".into()).is_err());
    assert!(clear(&mut im, (-1, 1, 3, 2), &"cols".into()).is_err());
    reflect(
        &mut im,
        &json!({"src": [0, 0, 3, 1], "dst_y": 2, "alpha": [1, 0]}),
    )
    .unwrap();
    assert!(
        reflect(
            &mut im,
            &json!({"src": [0, 0, 3, 1], "dst_y": 3, "alpha": [1, 0]})
        )
        .is_err()
    );
}

/// A label with no words keeps the original text, and a note says so: a blank title is not a translation.
#[test]
fn empty_scene_text_keeps_the_original() {
    let original = b"\0\x04\xE5\x8B\x9D\x88";
    for lab in [
        json!({"at": 1, "len": 4, "text": ""}),
        json!({"at": 1, "len": 4}),
    ] {
        let mut d = *original;
        let mut notes = vec![];
        retext(&mut d, &json!({"id": "t", "labels": [lab]}), &mut notes).unwrap();
        assert_eq!(&d, original, "the original bytes stay");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            super::super::tests::says(&notes[0], "text.note_picture_no_words"),
            "{:?}",
            notes[0]
        );
    }
}

/// The shipped recipes on the original files: `WCCF_PICTURES_FROM` = a folder of unpacked DDS and sbscene files
/// (never in the tree), each remade in English and Italian to `<name>.<lang>.<ext>` beside it, same size.
#[test]
#[ignore = "needs original pictures"]
fn shipped_recipes_redraw_original_pictures() {
    let dir = PathBuf::from(std::env::var_os("WCCF_PICTURES_FROM").unwrap());
    let sheets = super::super::tests::manifest_dir().join("../../editions/2017-18");
    let font = Font {
        lib: freetype::Library::init().unwrap(),
        data: std::rc::Rc::new(fs::read(font_file()).unwrap()),
    };
    let refh = font.line(100, "Hgjy").unwrap().height().unwrap();
    let mut done = 0;
    for f in fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()) {
        let name = f.to_string_lossy().into_owned();
        let Some(ext) = [".dds", ".sbscene"].into_iter().find(|e| name.ends_with(e)) else {
            continue;
        };
        let data = fs::read(&f).unwrap();
        for (it, lang) in [(false, "en"), (true, "it")] {
            let Some(r) = recipes(&sheets, it)
                .unwrap()
                .into_iter()
                .find(|r| r["sha256"].as_str() == Some(&sha(&data)))
            else {
                continue;
            };
            let out = remake(&data, &r, &font, refh, &name, &mut vec![]).unwrap();
            assert_eq!(out.len(), data.len(), "{name}");
            assert_eq!(out[..128], data[..128], "{name}: header changed");
            assert_ne!(out, data, "{name}: nothing changed");
            fs::write(
                format!("{}.{lang}{ext}", &name[..name.len() - ext.len()]),
                out,
            )
            .unwrap();
            done += 1;
        }
    }
    assert!(done > 0, "no original picture in {}", dir.display());
}

/// Titles as the game shows them (quads from the original scenes, 960 px screen): one phrase centred on 480
/// with one word space. CLUB HOUSE is two textures whose slices overlap at x 360.5-374 (`Title_tex_2` cols
/// 1-182 at 191 + x, `Title_tex_1` cols 0-406 at 360.5 + x), so each half draws the whole phrase and
/// `Title_tex_1` erases the strip `Title_tex_2` already shows.
#[test]
fn titles_are_one_phrase_centred_on_the_screen() {
    let sheets = super::super::tests::manifest_dir().join("../../editions/2017-18");
    let font = Font {
        lib: freetype::Library::init().unwrap(),
        data: std::rc::Rc::new(fs::read(font_file()).unwrap()),
    };
    let refh = font.line(100, "Hgjy").unwrap().height().unwrap();
    // (id, texture width, first and last column shown, screen x of column 0 doubled)
    type Slice = (&'static str, usize, usize, usize, i64);
    let titles: [&[Slice]; 2] = [
        &[
            ("title_team", 256, 1, 182, 382),
            ("title_management", 512, 0, 406, 721),
        ],
        &[("title_licence_entry", 512, 0, 510, 449)],
    ];
    for it in [false, true] {
        let rs = recipes(&sheets, it).unwrap();
        for title in titles {
            let mut on = std::collections::BTreeSet::new();
            for &(id, w, a, b, x2) in title {
                let r = rs.iter().find(|r| r["id"] == id).unwrap();
                let mut im = Img {
                    w,
                    h: 64,
                    px: vec![0; w * 64 * 4],
                };
                redraw(&mut im, r, &font, refh).unwrap();
                // the words, not their halo (at most alpha 190)
                on.extend(
                    (a..=b)
                        .filter(|&x| (0..64).any(|y| im.get(x, y)[3] > 200))
                        .map(|x| x2 + 2 * x as i64),
                );
            }
            let (lo, hi) = (*on.first().unwrap(), *on.last().unwrap());
            let what = format!("{} it={it}", title[0].0);
            assert!(
                (lo + hi - 1920).abs() <= 8,
                "{what}: centre {}",
                (lo + hi) as f64 / 4.0
            );
            let v: Vec<i64> = on.into_iter().collect();
            let gaps: Vec<i64> = v
                .windows(2)
                .map(|p| (p[1] - p[0]) / 2 - 1)
                .filter(|&g| g >= 8)
                .collect();
            assert!(
                gaps.len() == 1 && gaps[0] <= 20,
                "{what}: word gaps {gaps:?}"
            );
        }
    }
}

/// The seam of a title over two textures: the erased strip turns transparent but keeps the colour drawn
/// there, so the game's filtering across the seam blends the same colour and draws no grey line. Checked on the
/// redrawn pixels and again after the shipped path's BC3 (`encode`, read back as the game reads it).
#[test]
fn erase_keeps_the_colour_under_the_seam() {
    let sheets = super::super::tests::manifest_dir().join("../../editions/2017-18");
    let font = Font {
        lib: freetype::Library::init().unwrap(),
        data: std::rc::Rc::new(fs::read(font_file()).unwrap()),
    };
    let refh = font.line(100, "Hgjy").unwrap().height().unwrap();
    for it in [false, true] {
        let rs = recipes(&sheets, it).unwrap();
        let r = rs.iter().find(|r| r["id"] == "title_management").unwrap();
        let mut plain = r.clone();
        plain.as_object_mut().unwrap().remove("erase");
        let blank = || Img {
            w: 512,
            h: 64,
            px: vec![0; 512 * 64 * 4],
        };
        let (mut im, mut want) = (blank(), blank());
        redraw(&mut im, r, &font, refh).unwrap();
        redraw(&mut want, &plain, &font, refh).unwrap();
        let (x0, y0, x1, y1) = boxv(&r["erase"][0]);
        let strip =
            move || (y0..=y1).flat_map(move |y| (x0..=x1).map(move |x| (x as usize, y as usize)));
        assert!(
            strip().any(|(x, y)| want.get(x, y)[3] > 0),
            "it={it}: nothing drawn under the seam"
        );
        for (x, y) in strip() {
            let (p, q) = (im.get(x, y), want.get(x, y));
            assert!(
                p[3] == 0 && p[..3] == q[..3],
                "it={it} ({x}, {y}): {p:?}, drawn {q:?}"
            );
        }
        // the original Title_tex_1.dds: 512x64 DXT5, one level (mipMapCount 0), so no smaller level to check
        let mut orig = dds_header(512, 64, 0, b"DXT5", 0, [0; 4]);
        orig.resize(128 + 512 * 64, 0);
        let read = |im: &Img| bcn::decode_dds(&encode(&orig, im).unwrap()).unwrap().2;
        let (got, drawn) = (read(&im), read(&want));
        for (x, y) in strip() {
            let i = (y * 512 + x) * 4;
            let (p, q) = (&got[i..i + 4], &drawn[i..i + 4]);
            assert!(
                p[3] == 0 && p[..3] == q[..3],
                "it={it} after BC3 ({x}, {y}): {p:?}, drawn {q:?}"
            );
        }
    }
}

#[test]
fn subsets_by_id_or_file_name() {
    assert!(wanted(None).is_none() && wanted(Some("all")).is_none());
    assert_eq!(wanted(Some("none")), Some(HashSet::new()));
    let n = wanted(Some(" Pic , other"));
    assert!(chosen("x", "ui/pic.dds", n.as_ref()) && chosen("OTHER", "ui/a.dds", n.as_ref()));
    assert!(!chosen("x", "ui/a.dds", n.as_ref()));
}

#[test]
fn colour_is_the_median_of_the_brightest_opaque_pixels() {
    let px = [
        [10, 10, 10, 255],
        [200, 100, 50, 255],
        [250, 250, 250, 255],
        [255, 255, 255, 0],
        [90, 90, 90, 255],
    ];
    let im = Img {
        w: 5,
        h: 1,
        px: px.concat(),
    };
    // 4 opaque, the brightest 40% = 1 pixel
    assert_eq!(colour(&im, (0, 0, 4, 0), &"auto".into()), [250, 250, 250]);
    assert_eq!(colour(&im, (0, 0, 4, 0), &"dark".into()), [10, 10, 10]);
    assert_eq!(colour(&im, (0, 0, 4, 0), &json!([1, 2, 3])), [1, 2, 3]);
    assert_eq!(pyround(2.5), 2);
    assert_eq!(pyround(3.5), 4);
}

/// A scratch archive: a folder "ui" and its file "pic.dds" (YS packed) in sector 1.
struct Scratch {
    _t: tempfile::TempDir,
    game: PathBuf,
    kit: PathBuf,
    state: PathBuf,
    original: Vec<u8>,
    dds: Vec<u8>,
    entry: Vec<u8>,
    hdr: Vec<u8>,
}

impl Scratch {
    fn new(packed_dds: bool) -> Self {
        let t = tempfile::tempdir().unwrap();
        let (game, kit) = (t.path().join("game"), t.path().join("kit"));
        for d in [
            game.join("data"),
            kit.join("english"),
            kit.join("italian"),
            kit.join("fonts"),
        ] {
            fs::create_dir_all(d).unwrap();
        }
        fs::copy(font_file(), kit.join("fonts").join(FONT)).unwrap();
        // "the original writing": a white block
        let mut px = vec![0u8; 64 * 32 * 4];
        for y in 8..24 {
            for x in 8..56 {
                px[(y * 64 + x) * 4..][..4].copy_from_slice(&[255; 4]);
            }
        }
        let mut dds = dds_header(64, 32, 0, b"DXT5", 0, [0; 4]);
        dds.extend(bcn::encode_dxt5(&px, 64, 32));
        let stored = if packed_dds {
            ys_pack(&dds)
        } else {
            dds.clone()
        };
        let mut toc = vec![0u8; 2 * ENTRY];
        toc[..2].copy_from_slice(b"ui");
        toc[0x84..0x88].copy_from_slice(&(-1i32).to_le_bytes());
        let e = &mut toc[ENTRY..];
        e[..7].copy_from_slice(b"pic.dds");
        (e[0x80], e[0x81]) = (1, u8::from(packed_dds));
        e[0x84..0x88].copy_from_slice(&0i32.to_le_bytes());
        e[0x94..0x98].copy_from_slice(&(dds.len() as u32).to_le_bytes());
        e[0x98..0x9C].copy_from_slice(&(stored.len() as u32).to_le_bytes());
        e[0xA0..0xA4].copy_from_slice(&1u32.to_le_bytes());
        let mut hdr = vec![0u8; TOC_AT as usize];
        hdr[..4].copy_from_slice(b"xaf0");
        for (o, v) in [(4, 2u32), (8, SECTOR as u32), (12, 2)] {
            hdr[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        let sectors = 1 + stored.len().div_ceil(SECTOR as usize) as u64;
        for (o, v) in [(0x18, sectors), (0x20, 1), (0x28, sectors - 1)] {
            hdr[o..o + 8].copy_from_slice(&v.to_le_bytes());
        }
        let mut original = [hdr.clone(), toc.clone()].concat();
        original.resize(SECTOR as usize, 0);
        original.extend(&stored);
        original.resize(sectors as usize * SECTOR as usize, 0);
        fs::write(game.join(XAF), &original).unwrap();
        let s = Self {
            state: t.path().join("english_backup").join(STATE),
            entry: toc[ENTRY..].to_vec(),
            _t: t,
            game,
            kit,
            original,
            dds,
            hdr,
        };
        s.recipe(&sha(&s.dds));
        fs::write(
            s.kit.join("italian/pictures.json"),
            r#"{"test_pic": ["Sì"]}"#,
        )
        .unwrap();
        s
    }

    fn recipe(&self, h: &str) {
        let r = json!({"pictures": [{"id": "test_pic", "sha256": h, "files": ["ui/pic.dds"],
            "style": {"core": "auto", "halo": [0, 0, 0, 200], "halo_px": 1},
            "labels": [{"box": [6, 6, 57, 25], "en": "OK"}]}]});
        fs::write(self.kit.join("english/pictures.json"), r.to_string()).unwrap();
    }

    fn archive(&self) -> Vec<u8> {
        fs::read(self.game.join(XAF)).unwrap()
    }

    fn build(&self, it: bool, spec: &str) -> R<Pics> {
        build(&self.game, &self.kit, it, Some(spec), &self.state).map(|(p, _)| p)
    }
}

#[test]
fn on_adds_at_the_end_and_off_gives_back_the_original() {
    let s = Scratch::new(true);
    let pics = s.build(false, "all").unwrap();
    assert_eq!(
        pics.iter()
            .map(|(i, p, _)| (*i, p.as_str()))
            .collect::<Vec<_>>(),
        [(1, "ui/pic.dds")]
    );
    assert_eq!(apply(&s.game, &pics, &s.state).unwrap(), 1);
    let on = s.archive();
    let n = s.original.len();
    assert!(
        on.len() > n && on[SECTOR as usize..n] == s.original[SECTOR as usize..],
        "added at the end"
    );
    assert_eq!(
        u64::from_le_bytes(on[0x18..0x20].try_into().unwrap()),
        on.len() as u64 / SECTOR
    );
    let ne = &on[TOC_AT as usize + ENTRY..][..ENTRY];
    assert_eq!(
        u32le(ne, 0xA0) as usize,
        n / SECTOR as usize,
        "the entry points at the added picture"
    );
    let mut f = File::open(s.game.join(XAF)).unwrap();
    let new = read_entry(&mut f, ne).unwrap();
    assert_eq!((new.len(), &new[..128]), (s.dds.len(), &s.dds[..128]));
    let (_, _, px) = bcn::decode_dds(&new).unwrap();
    assert!(px[(9 * 64 + 9) * 4 + 3] < 100, "the old writing is cleared");
    assert!(px.chunks(4).any(|p| p[3] > 0), "the words are drawn");
    // the way back as Python's json.dump(indent=1)
    let st = fs::read_to_string(&s.state).unwrap();
    assert!(
        st.starts_with(&format!("{{\n \"length\": {n},\n \"header\": \"")),
        "{st}"
    );
    assert!(
        st.ends_with(&format!("\"1\": \"{}\"\n }}\n}}", to_hex(&s.entry))),
        "{st}"
    );

    assert_eq!(
        apply(&s.game, &s.build(false, "all").unwrap(), &s.state).unwrap(),
        1
    );
    assert_eq!(s.archive(), on, "on twice: the same file");
    assert_ne!(
        s.build(true, "all").unwrap()[0].2,
        pics[0].2,
        "Italian words differ"
    );
    assert_eq!(
        s.build(false, "pic").unwrap().len(),
        1,
        "a subset by file name"
    );
    assert!(s.build(false, "none").unwrap().is_empty());
    assert!(s.build(false, "no_such_picture").is_err());

    assert_eq!(restore(&s.game, &s.state).unwrap(), 1);
    assert_eq!(
        s.archive(),
        s.original,
        "off: the original archive byte for byte"
    );
    assert!(!s.state.exists() && restore(&s.game, &s.state).unwrap() == 0);
}

#[test]
fn off_after_an_interrupted_on_and_a_foreign_picture() {
    let s = Scratch::new(true);
    let pics = s.build(false, "all").unwrap();
    apply(&s.game, &pics, &s.state).unwrap();
    // stopped half way: the way back saved, the entry and header not yet written
    let mut f = OpenOptions::new()
        .write(true)
        .open(s.game.join(XAF))
        .unwrap();
    f.seek(SeekFrom::Start(TOC_AT + ENTRY as u64)).unwrap();
    f.write_all(&s.entry).unwrap();
    f.seek(SeekFrom::Start(0x18)).unwrap();
    f.write_all(&s.hdr[0x18..0x30]).unwrap();
    drop(f);
    assert_eq!(restore(&s.game, &s.state).unwrap(), 1);
    assert_eq!(s.archive(), s.original);
    s.recipe(&"00".repeat(32)); // not the recipe's picture: skipped, nothing written
    assert!(s.build(false, "all").unwrap().is_empty());
    assert_eq!(apply(&s.game, &vec![], &s.state).unwrap(), 0);
    assert_eq!(s.archive(), s.original);
}

#[test]
fn a_damaged_way_back_writes_nothing_and_stays() {
    let s = Scratch::new(true);
    apply(&s.game, &s.build(false, "all").unwrap(), &s.state).unwrap();
    let on = s.archive();
    let good = fs::read_to_string(&s.state).unwrap();
    let short = good.replace(&to_hex(&s.entry), &to_hex(&s.entry[..ENTRY - 1]));
    for bad in [short, good.replace("\"header\": \"", "\"header\": \"0")] {
        fs::write(&s.state, &bad).unwrap();
        assert!(restore(&s.game, &s.state).is_err());
        assert_eq!(s.archive(), on, "nothing written");
        assert!(s.state.is_file(), "the state stays");
    }
    // a huge entry count is refused, not allocated
    let mut huge = s.original.clone();
    huge[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    fs::write(s.game.join(XAF), &huge).unwrap();
    assert!(toc(&mut File::open(s.game.join(XAF)).unwrap()).is_err());
}

#[test]
fn a_short_or_unparsable_way_back_is_an_error_not_a_panic() {
    let s = Scratch::new(true);
    let st = json!({"length": s.original.len(), "header": to_hex(&s.hdr[0x18..0x30]), "entries": {"1": to_hex(&[0; 10])}});
    fs::create_dir_all(s.state.parent().unwrap()).unwrap();
    fs::write(&s.state, st.to_string()).unwrap();
    assert!(s.build(false, "all").is_err());
    fs::write(&s.state, b"{").unwrap();
    assert!(s.build(false, "all").is_err());
    assert!(
        restore(&s.game, &s.state).is_err(),
        "not \"nothing to restore\""
    );
    assert_eq!(s.archive(), s.original);
    assert!(s.state.is_file(), "the state stays");
}

#[test]
fn a_2010_11_state_is_taken_back() {
    // the 2010-11 script's way of adding a picture: stored as is, entry flags cleared, the same state file
    let s = Scratch::new(false);
    let blob = [s.dds.clone(), vec![7; 3000]].concat();
    let n = s.original.len() as u64;
    let mut e = s.entry.clone();
    (e[0x81], e[0x82]) = (0, 0);
    e[0x94..0x98].copy_from_slice(&(blob.len() as u32).to_le_bytes());
    e[0x98..0x9C].copy_from_slice(&(blob.len() as u32).to_le_bytes());
    e[0xA0..0xA4].copy_from_slice(&((n / SECTOR) as u32).to_le_bytes());
    let mut on = s.archive();
    on[TOC_AT as usize + ENTRY..][..ENTRY].copy_from_slice(&e);
    on.extend(&blob);
    on.resize(on.len().div_ceil(SECTOR as usize) * SECTOR as usize, 0);
    fs::write(s.game.join(XAF), &on).unwrap();
    let st = json!({"length": n, "header": to_hex(&s.hdr[0x18..0x30]), "entries": {"1": to_hex(&s.entry)}});
    fs::create_dir_all(s.state.parent().unwrap()).unwrap();
    fs::write(&s.state, serde_json::to_string_pretty(&st).unwrap()).unwrap();
    assert_eq!(restore(&s.game, &s.state).unwrap(), 1);
    assert_eq!(s.archive(), s.original);
    // an archive cut shorter than the way back says: left as it is
    fs::write(&s.state, st.to_string()).unwrap();
    fs::write(s.game.join(XAF), &s.original[..SECTOR as usize]).unwrap();
    assert!(restore(&s.game, &s.state).is_err() && s.state.exists());
}
