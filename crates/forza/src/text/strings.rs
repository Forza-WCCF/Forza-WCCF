use super::*;

// ---------------------------------------------------------------- bytes of the screen text
fn splitlines(b: &[u8]) -> Vec<&[u8]> {
    let (mut out, mut s, mut i) = (vec![], 0, 0);
    while i < b.len() {
        if b[i] == b'\n' || b[i] == b'\r' {
            i += if b[i] == b'\r' && b.get(i + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
            out.push(&b[s..i]);
            s = i;
        } else {
            i += 1;
        }
    }
    if s < b.len() {
        out.push(&b[s..]);
    }
    out
}

pub(super) fn words(line: &[u8]) -> Vec<&[u8]> {
    line.split(|b| b.is_ascii_whitespace())
        .filter(|w| !w.is_empty())
        .collect()
}

/// The NUL-ended string at `p` (Python's `body[p:body.find(b"\0", p)]`).
pub(super) fn at(body: &[u8], p: usize) -> &[u8] {
    let end = body
        .get(p..)
        .and_then(|t| t.iter().position(|&b| b == 0))
        .map_or(body.len().saturating_sub(1), |n| p + n);
    body.get(p..end).unwrap_or(&[])
}

pub(super) type Keys = HashMap<String, (usize, Vec<usize>)>;

pub(super) fn read_strings(raw: &[u8], hf_raw: &[u8]) -> R<(Vec<u8>, Keys)> {
    let mut keys = Keys::new();
    for (i, line) in splitlines(hf_raw).into_iter().enumerate().skip(1) {
        let w = words(line);
        let Some((k, nums)) = w.split_first() else {
            continue;
        };
        let nums = nums
            .iter()
            .map(|x| std::str::from_utf8(x).ok().and_then(|x| x.parse().ok()))
            .collect::<Option<Vec<usize>>>();
        // slot 0 (Japanese) and 1 are read for every key: a line with fewer numbers is damaged
        let (Some(nums), Ok(k)) = (nums.filter(|n| n.len() >= 2), std::str::from_utf8(k)) else {
            return fail(Line::key("text.fail_hf_line"));
        };
        keys.insert(k.into(), (i, nums));
    }
    Ok((raw.get(8..).unwrap_or(&[]).to_vec(), keys))
}

fn blocked(k: &str) -> bool {
    let f = k.splitn(3, '_').take(2).collect::<Vec<_>>().join("_");
    DATA_FAMILIES.iter().any(|d| f.starts_with(d)) && !SWITCHED_ON.iter().any(|e| f.starts_with(e))
}

pub(super) fn cp932(b: &[u8]) -> String {
    encoding_rs::SHIFT_JIS
        .decode_without_bom_handling(b)
        .0
        .into_owned()
}

fn has_high(b: &[u8]) -> bool {
    b.iter().any(|&c| c >= 0x80)
}

/// The line as the game's bytes, or what is wrong with it: a line keeps exactly the printf and `$` codes of the
/// game's Japanese line, the printf ones in its order (msvcr90's `vsprintf_s` takes no positional argument).
pub(super) fn encoded(ed: Edition, jp: &str, text: &str, nl: &[u8]) -> Result<Vec<u8>, Line> {
    let sorted = |r: &Regex, s: &str| {
        let mut v: Vec<String> = r.find_iter(s).map(|m| m.as_str().into()).collect();
        v.sort();
        v
    };
    // In the order they come, never sorted: the game reaches its values by position.
    let fmt = |s: &str| {
        FMT.find_iter(s)
            .map(|m| m.as_str().to_string())
            .collect::<Vec<_>>()
    };
    if text.trim() == "<empty>" {
        let codes = match ed {
            Edition::E1011 => jp.contains("$c") || jp.contains("$p"),
            Edition::E1718 => CODES.is_match(jp),
        };
        return if FMT.is_match(jp) || codes {
            Err(Line::key("text.why_empty"))
        } else {
            Ok(vec![])
        };
    }
    let en = text.replace("\\n", "\n");
    if fmt(jp) != fmt(&en) {
        return Err(Line::key("text.why_codes")
            .arg("found", format!("{:?}", fmt(&en)))
            .arg("want", format!("{:?}", fmt(jp))));
    }
    if FMT.replace_all(&en, "").matches('%').count() > FMT.replace_all(jp, "").matches('%').count()
    {
        return Err(Line::key("text.why_percent"));
    }
    match ed {
        Edition::E1011 => {
            // every $ code letter ($c $p $t ...) as often as the game's line has it; a colour's value may differ
            let letters = |s: &str| {
                let mut v: Vec<String> = CODES
                    .find_iter(s)
                    .map(|m| m.as_str()[..2].to_string())
                    .collect();
                v.sort();
                v
            };
            if jp.matches("$c[").count() != en.matches("$c[").count() || letters(jp) != letters(&en)
            {
                return Err(Line::key("text.why_dollar"));
            }
        }
        Edition::E1718 => {
            if sorted(&CODES, jp) != sorted(&CODES, &en) {
                return Err(Line::key("text.why_dollar_diff")
                    .arg("found", format!("{:?}", sorted(&CODES, &en)))
                    .arg("want", format!("{:?}", sorted(&CODES, jp))));
            }
        }
    }
    let (b, _, bad) = encoding_rs::SHIFT_JIS.encode(&en);
    if bad {
        return Err(Line::key("text.why_letter"));
    }
    let mut b = b.into_owned();
    if ed == Edition::E1011 && nl != b"\n" {
        b = b.split(|&c| c == b'\n').collect::<Vec<_>>().join(nl);
    }
    if b.contains(&0) {
        return Err(Line::key("text.why_end_byte"));
    }
    Ok(b)
}

/// Where a line's words came from. The order is the note's (`by_line`): the labels' old alphabetical order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Source {
    Kit,
    KitItalian,
    OriginalEnglish,
    OriginalEuItalian,
    OriginalItalian,
    OriginalSecond,
}

impl Source {
    /// The kit's own sheets, not the original's lines.
    pub(super) fn kit(self) -> bool {
        matches!(self, Self::Kit | Self::KitItalian)
    }

    /// Its label in the note under the lines (`text.src_*`).
    pub(super) fn line(self) -> Line {
        Line::key(match self {
            Self::Kit => "text.src_kit",
            Self::KitItalian => "text.src_kit_it",
            Self::OriginalSecond => "text.src_original_second",
            Self::OriginalEnglish => "text.src_original_english",
            Self::OriginalItalian => "text.src_original_it",
            Self::OriginalEuItalian => "text.src_original_eu_it",
        })
    }
}

pub(super) type Choices = BTreeMap<String, Vec<(String, Source)>>;

impl Ctx {
    /// Each Japanese line's choices, best first. 2010-11: one English line per key (the kit's, else the original's
    /// second column, else the original European English), Italian in front of it. 2017-18: the kit's Italian, the
    /// original's Italian column of the place names (`MAKE_city`, `MAKE_area`), the kit's English, then the
    /// original's as the next choices.
    pub(super) fn choices(&self, body: &[u8], keys: &Keys, lang: Lang) -> R<Choices> {
        let mut ch = Choices::new();
        let mut kit = HashSet::new();
        for r in self.sheet("screen_text.tsv", "english")? {
            if !cell(&r, "english").trim().is_empty() && kit.insert(cell(&r, "key").to_string()) {
                ch.insert(
                    cell(&r, "key").into(),
                    vec![(cell(&r, "english").trim_matches('\r').into(), Source::Kit)],
                );
            }
        }
        let rstring: HashMap<String, String> = self
            .sheet("original_rstring.tsv", "english")?
            .into_iter()
            .map(|r| (cell(&r, "key").into(), cell(&r, "english").into()))
            .collect();
        let e1718 = self.ed == Edition::E1718;
        let mut sorted_keys: Vec<_> = keys.iter().collect();
        sorted_keys.sort_by(|a, b| a.0.cmp(b.0));
        let from_original = |ch: &mut Choices| {
            for (k, (_, nums)) in &sorted_keys {
                if (!e1718 && kit.contains(*k)) || !has_high(at(body, nums[0])) {
                    continue;
                }
                let s2 = at(body, nums[1]);
                let c = if !s2.is_empty() && !has_high(s2) {
                    (
                        String::from_utf8_lossy(s2).into_owned(),
                        Source::OriginalSecond,
                    )
                } else if let Some(r) = rstring.get(*k) {
                    (r.clone(), Source::OriginalEnglish)
                } else {
                    continue;
                };
                ch.entry((*k).clone()).or_default().push(c);
            }
        };
        let italian = |ch: &mut Choices, folder_sheet: &str, src: Source| -> R<()> {
            for r in self.sheet(folder_sheet, "italian")? {
                if !cell(&r, "italiano").trim().is_empty() {
                    ch.entry(cell(&r, "key").into())
                        .or_default()
                        .insert(0, (cell(&r, "italiano").trim_matches('\r').into(), src));
                }
            }
            Ok(())
        };
        if e1718 {
            if lang == Lang::It {
                italian(&mut ch, "screen_text.tsv", Source::KitItalian)?;
                // the original's own Italian column for the place names (Barcellona, Monaco di Baviera), before the
                // English; the kit's Italian stays first
                for (k, (_, nums)) in &sorted_keys {
                    let s3 = nums.get(2).map_or(&[][..], |&n| at(body, n));
                    if ["MAKE_city", "MAKE_area"].iter().any(|f| k.starts_with(f))
                        && !s3.is_empty()
                        && !has_high(s3)
                        && has_high(at(body, nums[0]))
                    {
                        let e = ch.entry((*k).clone()).or_default();
                        let i = usize::from(e.first().is_some_and(|c| c.1 == Source::KitItalian));
                        e.insert(
                            i,
                            (
                                String::from_utf8_lossy(s3).into_owned(),
                                Source::OriginalItalian,
                            ),
                        );
                    }
                }
            }
            from_original(&mut ch);
        } else {
            from_original(&mut ch);
            if lang == Lang::It {
                italian(&mut ch, "original_rstring.tsv", Source::OriginalEuItalian)?;
                italian(&mut ch, "screen_text.tsv", Source::KitItalian)?;
            }
        }
        Ok(ch)
    }
}

/// {key: bytes} and {key: source} for every line with a choice that fits, the first that does; the kit's lines that
/// did not fit (they give way to the next choice), and the unusable original ones.
/// Offsets of the packed strings, the keys that spilled, and the free (start, end) gaps.
type Slots<'a> = (BTreeMap<String, usize>, Vec<&'a String>, Vec<(i64, i64)>);
/// What `encode_lines` returns: encoded lines, the choice taken per key, the keys that did not fit, a count.
type Encoded = (
    BTreeMap<String, Vec<u8>>,
    HashMap<String, Source>,
    Vec<Line>,
    usize,
);
pub(super) fn encode_lines(
    ed: Edition,
    body: &[u8],
    keys: &Keys,
    ch: &Choices,
    nl: &[u8],
) -> Encoded {
    let (mut enc, mut used, mut misfit, mut original_bad) =
        (BTreeMap::new(), HashMap::new(), vec![], 0);
    for (k, options) in ch {
        let Some((_, nums)) = keys.get(k) else {
            misfit.extend(options.iter().filter(|(_, s)| s.kit()).map(|_| {
                Line::key("text.misfit")
                    .arg("key", k)
                    .part("why", Line::key("text.why_no_line"))
            }));
            continue;
        };
        if blocked(k) {
            continue;
        }
        let jp = cp932(at(body, nums[0]));
        for (text, src) in options {
            match encoded(ed, &jp, text, nl) {
                Ok(b) => {
                    enc.insert(k.clone(), b);
                    used.insert(k.clone(), *src);
                    break;
                }
                Err(p) if src.kit() => misfit.push(if ed == Edition::E1718 {
                    Line::key("text.misfit_source")
                        .arg("key", k)
                        .part("source", src.line())
                        .part("why", p)
                } else {
                    Line::key("text.misfit").arg("key", k).part("why", p)
                }),
                Err(_) => original_bad += 1,
            }
        }
    }
    (enc, used, misfit, original_bad)
}

/// `%{source} %{count}` for each source, in order: one key for the first, one for each after it.
fn by_line(by: &BTreeMap<Source, usize>) -> Line {
    let mut it = by.iter();
    let Some((src, n)) = it.next() else {
        return Line::raw("");
    };
    let mut out = Line::key("text.note_by")
        .part("source", src.line())
        .arg("count", n);
    for (src, n) in it {
        out = Line::key("text.note_by_more")
            .part("head", out)
            .part("source", src.line())
            .arg("count", n);
    }
    out
}

/// New string_list.bin, .hf and the offset of each packed key.
type Packed = (Vec<u8>, Vec<u8>, BTreeMap<String, usize>);
/// string_list.bin/.hf with slot 0 (Japanese) of each key in `enc` replaced; same size, read back.
pub(super) fn pack_strings(
    raw: &[u8],
    hf_raw: &[u8],
    enc: &BTreeMap<String, Vec<u8>>,
) -> R<Packed> {
    let (body, keys) = read_strings(raw, hf_raw)?;
    let hf_lines = splitlines(hf_raw);
    let mut refs: HashMap<usize, HashSet<(&str, usize)>> = HashMap::new();
    for (k, (_, nums)) in &keys {
        for (s, n) in nums.iter().enumerate() {
            refs.entry(*n).or_default().insert((k, s));
        }
    }
    let mut ref_sorted: Vec<usize> = refs.keys().copied().collect();
    ref_sorted.sort_unstable();
    let mut own: HashMap<&str, (usize, usize)> = HashMap::new();
    for k in enc.keys() {
        let Some((_, nums)) = keys.get(k) else {
            return fail(Line::key("text.fail_no_line").arg("key", k));
        };
        let p = nums[0];
        let end = p + at(&body, p).len() + 1;
        let (lo, hi) = (
            ref_sorted.partition_point(|&x| x < p),
            ref_sorted.partition_point(|&x| x < end),
        );
        if refs[&p].len() == 1 && refs[&p].contains(&(k.as_str(), 0)) && hi - lo == 1 {
            own.insert(k, (p, end));
        }
    }
    let used_len = body.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    let pad_at = (used_len + 1).max(ref_sorted.last().map_or(0, |l| l + 1)) + 16;
    let mut nbody = body.clone();
    for &(p, end) in own.values() {
        nbody[p..end].fill(0);
    }
    let (mut pos, mut spill, mut free): Slots<'_> = (BTreeMap::new(), vec![], vec![]);
    let mut order: Vec<_> = enc.iter().collect();
    order.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    for (k, b) in order {
        match own.get(k.as_str()) {
            Some(&(p, end)) if b.len() < end - p => {
                nbody[p..p + b.len()].copy_from_slice(b);
                pos.insert(k.clone(), p);
                if end as i64 - (p + b.len() + 1) as i64 > 1 {
                    free.push(((p + b.len() + 1) as i64, end as i64));
                }
            }
            o => {
                spill.push(k);
                if let Some(&(p, end)) = o {
                    free.push((p as i64, end as i64));
                }
            }
        }
    }
    free.push((pad_at as i64, body.len() as i64 - 1));
    for k in spill {
        let b = &enc[k];
        let Some(i) = free.iter().position(|&(s, e)| e - s > b.len() as i64) else {
            return fail(Line::key("text.fail_no_room").arg("key", k));
        };
        let s = free[i].0 as usize;
        nbody[s..s + b.len()].copy_from_slice(b);
        pos.insert(k.clone(), s);
        free[i].0 = (s + b.len() + 1) as i64;
    }
    let mut new_bin = raw[..4].to_vec();
    new_bin.extend((nbody.len() as u32).to_le_bytes());
    new_bin.extend(&nbody);
    let mut new_hf: Vec<Vec<u8>> = hf_lines.iter().map(|l| l.to_vec()).collect();
    for (k, p) in &pos {
        let i = keys[k].0;
        let l = hf_lines[i];
        let a = l.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(0);
        let a = a + l[a..]
            .iter()
            .position(|b| b.is_ascii_whitespace())
            .unwrap_or(0);
        let a = a + l[a..]
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(0);
        let z = a + l[a..]
            .iter()
            .position(|b| !b.is_ascii_digit())
            .unwrap_or(l.len() - a);
        new_hf[i] = [&l[..a], p.to_string().as_bytes(), &l[z..]].concat();
    }
    let mut wrong = 0;
    for line in new_hf.iter().skip(1) {
        let w = words(line);
        let Some((k, nums)) = w.split_first() else {
            continue;
        };
        let k = String::from_utf8_lossy(k).into_owned();
        let nums: Vec<usize> = nums
            .iter()
            .filter_map(|x| std::str::from_utf8(x).ok()?.parse().ok())
            .collect();
        let old = &keys[&k].1;
        if nums[1..] != old[1..] {
            wrong += 1;
        }
        for (s, n) in nums.iter().enumerate() {
            let want = if s == 0 && pos.contains_key(&k) {
                &enc[&k][..]
            } else {
                at(&body, old[s])
            };
            if at(&nbody, *n) != want {
                wrong += 1;
            }
        }
    }
    if wrong > 0 || new_bin.len() != raw.len() || new_hf[0] != hf_lines[0] {
        return fail(
            Line::key("text.fail_read_back_places")
                .part("what", Line::key("text.what_screen"))
                .arg("places", wrong),
        );
    }
    Ok((new_bin, new_hf.concat(), pos))
}

/// StringListTableRecord.bin: u32 count, then per line u32 id + 8 x (u32 length, cp932 bytes, no end byte): key, the
/// original comment, japanese, english, italian, spanish, french, german.
pub(super) type Rec = (u32, Vec<Vec<u8>>);

/// The `n` bytes at `*p` of the table `file`, `*p` moved past them; a table cut short fails.
pub(super) fn take<'a>(d: &'a [u8], p: &mut usize, n: usize, file: &str) -> R<&'a [u8]> {
    let b = d
        .get(*p..p.saturating_add(n))
        .ok_or_else(|| Failed(Line::key("text.fail_table_cut").arg("file", file)))?;
    *p += n;
    Ok(b)
}

/// A little-endian u32 taken as [`take`] does.
pub(super) fn take_u32(d: &[u8], p: &mut usize, file: &str) -> R<u32> {
    Ok(bcn::u32le(take(d, p, 4, file)?, 0))
}

/// A string taken as [`take`] does: its u32 length, then its bytes.
pub(super) fn take_str<'a>(d: &'a [u8], p: &mut usize, file: &str) -> R<&'a [u8]> {
    let n = take_u32(d, p, file)? as usize;
    take(d, p, n, file)
}

pub(super) fn read_enum(d: &[u8]) -> R<Vec<Rec>> {
    let (mut p, mut recs) = (0, vec![]);
    for _ in 0..take_u32(d, &mut p, ENUM)? {
        let id = take_u32(d, &mut p, ENUM)?;
        let f = (0..8)
            .map(|_| take_str(d, &mut p, ENUM).map(<[u8]>::to_vec))
            .collect::<R<_>>()?;
        recs.push((id, f));
    }
    if p != d.len() {
        return fail(
            Line::key("text.fail_table_tail")
                .arg("file", ENUM)
                .arg("count", d.len() - p),
        );
    }
    Ok(recs)
}

pub(super) fn write_enum(recs: &[Rec]) -> Vec<u8> {
    let mut out = (recs.len() as u32).to_le_bytes().to_vec();
    for (id, f) in recs {
        out.extend(id.to_le_bytes());
        for x in f {
            out.extend((x.len() as u32).to_le_bytes());
            out.extend(x);
        }
    }
    out
}

/// The Japanese column of each key in `enc` replaced, only where it is the server file's (the codes were checked
/// against that); read back. Returns the file and how many lines changed.
pub(super) fn pack_enum(
    enum_raw: &[u8],
    body: &[u8],
    keys: &Keys,
    enc: &BTreeMap<String, Vec<u8>>,
) -> R<(Vec<u8>, usize)> {
    let old = read_enum(enum_raw)?;
    let mut recs = old.clone();
    let mut done = HashMap::new();
    for (_, f) in &mut recs {
        let k = String::from_utf8_lossy(&f[0]).into_owned();
        if let (Some(b), Some((_, nums))) = (enc.get(&k), keys.get(&k))
            && f[2] == at(body, nums[0])
        {
            f[2] = b.clone();
            done.insert(k, b.clone());
        }
    }
    let new = write_enum(&recs);
    let back = read_enum(&new)?;
    let ok = back.len() == old.len()
        && back.iter().zip(&old).all(|((i, b), (j, o))| {
            let k = String::from_utf8_lossy(&o[0]).into_owned();
            i == j && b[..2] == o[..2] && b[3..] == o[3..] && b[2] == *done.get(&k).unwrap_or(&o[2])
        });
    if !ok {
        return fail(Line::key("text.fail_read_back").part("what", Line::key("text.what_client")));
    }
    Ok((new, done.len()))
}

impl Ctx {
    pub(super) fn build_strings(&self, lang: Lang) -> R<Built> {
        let (raw, hf_raw) = (self.original_file(BIN)?, self.original_file(HF)?);
        let enum_raw = if self.ed == Edition::E1718 {
            Some(self.original_file(ENUM)?)
        } else {
            None
        };
        for (rel, d) in [
            (BIN, Some(&raw)),
            (HF, Some(&hf_raw)),
            (ENUM, enum_raw.as_ref()),
        ] {
            if let Some(d) = d
                && Some(sha(d)) != self.original_hash(rel)
            {
                return fail(match self.ed {
                    Edition::E1011 => Line::key("text.fail_lines_not_original"),
                    Edition::E1718 => Line::key("text.fail_not_original_only")
                        .arg("file", rel)
                        .arg("edition", "2.00.00"),
                });
            }
        }
        let (body, keys) = read_strings(&raw, &hf_raw)?;
        let nl: &[u8] = if body.windows(2).any(|w| w == b"\r\n") {
            b"\r\n"
        } else {
            b"\n"
        };
        let ch = self.choices(&body, &keys, lang)?;
        let (enc, used, misfit, original_bad) = encode_lines(self.ed, &body, &keys, &ch, nl);
        if self.ed == Edition::E1011 && !misfit.is_empty() {
            let mut examples = misfit[0].clone();
            for next in misfit.iter().skip(1).take(2) {
                examples = Line::key("text.fail_example_more")
                    .part("head", examples)
                    .part("next", next.clone());
            }
            return fail(
                Line::key("text.fail_lines_problems")
                    .arg("count", misfit.len())
                    .part("examples", examples),
            );
        }
        let (new_bin, new_hf, pos) = pack_strings(&raw, &hf_raw, &enc)?;
        let mut by: BTreeMap<Source, usize> = BTreeMap::new();
        for k in pos.keys() {
            *by.entry(used[k]).or_default() += 1;
        }
        let mut files = vec![(BIN.to_string(), new_bin), (HF.to_string(), new_hf)];
        // The note under the lines: its head and every piece are ui.yml keys, the pieces that do not apply empty.
        let no = Line::raw("");
        let (mut clients, mut misfit_note) = (no.clone(), no.clone());
        let mut unusable = no.clone();
        if let Some(e) = enum_raw {
            let (new_enum, done) = pack_enum(&e, &body, &keys, &enc)?;
            clients = Line::key("text.note_clients").arg("count", done);
            files.push((ENUM.to_string(), new_enum));
            if !misfit.is_empty() {
                misfit_note = Line::key("text.note_misfit")
                    .arg("count", misfit.len())
                    .part("first", misfit[0].clone());
            }
        } else if original_bad > 0 {
            unusable = Line::key("text.note_unusable").arg("count", original_bad);
        }
        // Both editions: the real-game test looks for this note, and a line left in Japanese is wrong.
        let mut left: Vec<_> = keys
            .iter()
            .filter(|(k, (_, nums))| {
                !pos.contains_key(*k)
                    && !blocked(k)
                    && has_high(at(&body, nums[0]))
                    && !["SYS_DEBUG", "COM_debug"].contains(
                        &k.splitn(3, '_')
                            .take(2)
                            .collect::<Vec<_>>()
                            .join("_")
                            .as_str(),
                    )
            })
            .map(|(k, _)| k.as_str())
            .collect();
        left.sort_unstable();
        let mut japanese = no;
        if !left.is_empty() {
            japanese = Line::key("text.note_japanese")
                .arg("count", left.len())
                .arg(
                    "names",
                    format!(
                        "{}{}",
                        left[..left.len().min(3)].join(", "),
                        if left.len() > 3 { ", ..." } else { "" }
                    ),
                );
        }
        let note = Line::key("text.note_lines")
            .arg("count", pos.len())
            .part("lang", lang.line())
            .part("by", by_line(&by))
            .part("clients", clients)
            .part("misfit", misfit_note)
            .part("unusable", unusable)
            .part("japanese", japanese);
        Ok((files, vec![note]))
    }
}
