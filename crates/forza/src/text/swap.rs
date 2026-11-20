use super::*;

// ---------------------------------------------------------------- swapping files in and out
/// A manifest key: the platform's separator, as the Python scripts write it.
pub(super) fn key(rel: &str) -> String {
    rel.replace('/', MAIN_SEPARATOR_STR)
}

pub(super) fn put(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let mut t = tempfile::NamedTempFile::new_in(dir)?;
    t.write_all(data)?;
    t.as_file().sync_all()?; // on disk before the rename: a power cut never leaves an empty file under the name
    t.persist(path).map(drop).map_err(|e| e.error)
}

/// A manifest key with `folder\..\` taken out: the 0.3.2 Launcher, building the text with 0.3.3's sheets during
/// the update, keyed the guest teams `prog_data\cpu_team\..\guest_team\cpu_N.dat`; later kits look them up as
/// `prog_data\guest_team\cpu_N.dat` and refused their own English as "neither original nor backed up" (0.5.0).
fn fold_dots(k: &str) -> String {
    let mut out: Vec<&str> = vec![];
    for p in k.split(['\\', '/']) {
        match out.last() {
            Some(l) if p == ".." && *l != ".." => {
                out.pop();
            }
            _ => out.push(p),
        }
    }
    out.join(MAIN_SEPARATOR_STR)
}

/// The original's fingerprint in a backup-list entry (beside "english"): older kits wrote it under another key.
pub(super) fn original_of(h: &Value) -> Option<&str> {
    let m = h.as_object()?;
    m.get("original")
        .or_else(|| m.iter().find(|(k, _)| *k != "english").map(|(_, v)| v))?
        .as_str()
}

/// A backup list, its keys folded (`..` resolved); empty when missing, an error when unreadable or damaged (a
/// damaged list must never read as "nothing backed up").
pub(super) fn read_manifest(p: &Path) -> R<Map<String, Value>> {
    let d = match fs::read(p) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Map::new()),
        r => r?,
    };
    let m = serde_json::from_slice::<Map<String, Value>>(&d).map_err(|e| {
        Failed(
            Line::key("text.fail_backup_list_damaged")
                .arg("file", p.display())
                .arg("detail", e),
        )
    })?;
    Ok(m.into_iter().map(|(k, v)| (fold_dots(&k), v)).collect())
}

impl Ctx {
    fn manifest_path(&self) -> PathBuf {
        self.backup.join("manifest.json")
    }

    pub(super) fn load_manifest(&self) -> R<Map<String, Value>> {
        read_manifest(&self.manifest_path())
    }

    /// A damaged list over the original text names nothing the game still needs: kept aside as
    /// `manifest.json.bad`, so the next "on" backs the originals up again and "off" (UNDO SETUP) goes through.
    pub(super) fn set_aside_damaged_list(&self) -> R<()> {
        if self.load_manifest().is_err() && self.text_is_original() {
            let p = self.manifest_path();
            fs::rename(&p, p.with_extension("json.bad"))?;
        }
        Ok(())
    }

    fn save_manifest(&self, m: &Map<String, Value>) -> R<()> {
        let mut out = vec![];
        let mut ser = serde_json::Serializer::with_formatter(
            &mut out,
            serde_json::ser::PrettyFormatter::with_indent(b" "),
        );
        serde::Serialize::serialize(m, &mut ser).map_err(|e| Failed(Line::raw(e)))?;
        Ok(put(&self.manifest_path(), &out)?)
    }

    /// The original file copied into the backup (a program stored as the original WINMM import) and read back; it
    /// stays in place until its replacement lands in one step.
    fn backup_copy(&self, rel: &str, dst: &Path) -> R<()> {
        let data = norm(fs::read(self.at(rel))?, rel)?;
        put(dst, &data)?;
        if fs::read(dst)? != data {
            return fail(Line::key("text.fail_copy_backup").arg("file", rel));
        }
        Ok(())
    }

    /// Back each file up (the first time), then replace it in ONE step; the game's file is never missing.
    pub(super) fn turn_on(&self, files: &[(String, Vec<u8>)], lang: Lang) -> R<()> {
        let mut man = self.load_manifest()?;
        // not listed, or its backup gone (an "off" stopped half way): a backup is needed
        let unsaved = |man: &Map<String, Value>, rel: &str| {
            !man.contains_key(&key(rel)) || !self.backup.join(rel).is_file()
        };
        for (rel, _) in files {
            if unsaved(&man, rel)
                && Some(sha(&norm(fs::read(self.at(rel))?, rel)?)) != self.original_hash(rel)
            {
                return fail(Line::key("text.fail_no_backup").arg("file", rel));
            }
        }
        for (rel, data) in files {
            let tgt = self.at(rel);
            if unsaved(&man, rel) {
                self.backup_copy(rel, &self.backup.join(rel))?;
                let mut e = Map::new();
                e.insert("original".into(), self.original_hash(rel).into());
                man.insert(key(rel), Value::Object(e));
                self.save_manifest(&man)?; // after each backup: an interruption never loses one
            }
            put(&tgt, data)?;
            if sha(&fs::read(&tgt)?) != sha(data) {
                return fail(Line::key("text.fail_not_written").arg("file", rel));
            }
            if let Some(Value::Object(e)) = man.get_mut(&key(rel)) {
                e.insert("english".into(), sha(data).into());
            }
            apply_hook(rel, &tgt)?;
        }
        self.save_manifest(&man)?;
        put(
            &self.backup.join(SHEETS),
            format!("{}\n", sheets_hash(self.ed, &self.sheets)).as_bytes(),
        )?;
        Ok(put(
            &self.backup.join("language.txt"),
            format!("{}\n", lang.code()).as_bytes(),
        )?)
    }

    /// The originals back, one step each. Returns how many files the manifest named.
    pub(super) fn turn_off(&self) -> R<usize> {
        let man = self.load_manifest()?;
        let mut lost = vec![];
        for (k, h) in &man {
            let rel = k.replace('\\', "/");
            let (bak, tgt) = (self.backup.join(&rel), self.at(&rel));
            let want = original_of(h).unwrap_or("");
            if bak.is_file() {
                // a program's backup as the original WINMM import (an older kit's raw copy is the same bytes)
                let data = norm(fs::read(&bak)?, &rel)?;
                if sha(&data) != want {
                    return fail(Line::key("text.fail_backup_not_original").arg("file", rel));
                }
                put(&tgt, &data)?; // one step: the original file back, never a moment without one
                apply_hook(&rel, &tgt)?;
                fs::remove_file(&bak)?;
            } else if tgt.is_file() && sha(&norm(fs::read(&tgt)?, &rel)?) != want {
                lost.push(rel); // no backup: put back already, or lost
            }
        }
        if !lost.is_empty() {
            return fail(
                Line::key("text.fail_no_original_backup")
                    .arg("count", lost.len())
                    .arg("first", lost[0].clone())
                    .arg("more", if lost.len() > 1 { ", ..." } else { "" }),
            );
        }
        for f in [
            self.manifest_path(),
            self.backup.join("language.txt"),
            self.backup.join(SHEETS),
        ] {
            if f.is_file() {
                fs::remove_file(f)?;
            }
        }
        prune(&self.backup);
        Ok(man.len())
    }
}

/// 2017-18: the backup is also kept here, next to the game (`original\text`, beside `game\` and the original
/// programs), so a Forza-WCCF folder that never switched this game still finds the originals.
pub(super) fn beside(game: &Path) -> PathBuf {
    game.parent().unwrap_or(game).join("original").join("text")
}

/// What [`sync_beside`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum Beside {
    /// Nothing to do, or the backup beside the game was kept or taken into this folder's own backup.
    Fine,
    /// The game text is not original and no backup that matches the original files exists: the text stays as it is.
    Foreign,
}

/// Every file under `from` to `to` except those named in `skip`.
fn copy_tree(from: &Path, to: &Path, skip: &[&str]) -> io::Result<()> {
    for e in fs::read_dir(from)? {
        let e = e?;
        let (p, name) = (e.path(), e.file_name());
        if p.is_dir() {
            copy_tree(&p, &to.join(&name), skip)?;
        } else if !skip.iter().any(|s| name == *s) {
            put(&to.join(&name), &fs::read(&p)?)?;
        }
    }
    Ok(())
}

/// The game's `string_list.bin` is the pinned original (or absent): one file hashed (about 1 MB).
pub(crate) fn text_is_original(ed: Edition, game: &Path) -> bool {
    let (rel, want) = spec(ed).originals[STRINGS];
    !fs::read(game.join(rel)).is_ok_and(|d| sha(&d) != want)
}

impl Ctx {
    fn text_is_original(&self) -> bool {
        text_is_original(self.ed, &self.game)
    }

    /// Copy this folder's backup beside the game, the manifest last: a copy cut short is not used. Entries
    /// another folder left there stay listed while their file is there: their originals are nowhere else.
    pub(super) fn save_beside(&self) -> R<()> {
        let to = beside(&self.game);
        // a damaged list there is replaced: this folder's own list is the one that counts
        let mut man = read_manifest(&to.join("manifest.json")).unwrap_or_default();
        man.retain(|k, _| to.join(k.replace('\\', "/")).is_file());
        // the old list stays until the new one lands in one step: the files it names stay originals meanwhile
        copy_tree(&self.backup, &to, &["manifest.json", SHEETS])?;
        man.extend(self.load_manifest()?);
        let out = serde_json::to_vec_pretty(&man).map_err(|e| Failed(Line::raw(e)))?;
        Ok(put(&to.join("manifest.json"), &out)?)
    }

    /// Take the backup beside the game as this folder's own: each file this folder's sheets pin must be that
    /// original (a wrong one refuses the whole backup); files they do not name are another kit's and stay out.
    /// The pictures' way back comes along only when it matches the archive. No `sheets.txt`: the text counts as
    /// older and the next step builds it again from these sheets.
    fn adopt_beside(&self) -> Beside {
        let from = beside(&self.game);
        let mut keep = vec![];
        // a damaged list there backs nothing up: Foreign, nothing changed
        for (k, h) in read_manifest(&from.join("manifest.json")).unwrap_or_default() {
            let rel = k.replace('\\', "/");
            let Some(want) = self.original_hash(&rel) else {
                continue;
            };
            match norm_read(&from.join(&rel), &rel) {
                Some(d)
                    if sha(&d) == want
                        && original_of(&h).is_some_and(|o| o.eq_ignore_ascii_case(&want)) =>
                {
                    keep.push((k, h, rel, d));
                }
                _ => return Beside::Foreign,
            }
        }
        if keep.is_empty() {
            return Beside::Foreign;
        }
        let lang = fs::read(from.join("language.txt")).unwrap_or_else(|_| b"en\n".to_vec());
        let state = from.join(pictures::STATE);
        let done = (|| -> R<()> {
            for (_, _, rel, d) in &keep {
                put(&self.backup.join(rel), d)?;
            }
            put(&self.backup.join("language.txt"), &lang)?;
            if pictures::state_matches(&self.game, &state) {
                put(&self.backup.join(pictures::STATE), &fs::read(&state)?)?;
            }
            self.save_manifest(&keep.into_iter().map(|(k, h, ..)| (k, h)).collect())
        })();
        if done.is_ok() {
            Beside::Fine
        } else {
            Beside::Foreign
        }
    }
}

/// A file read and normalised as its backup is kept, or `None`.
fn norm_read(p: &Path, rel: &str) -> Option<Vec<u8>> {
    norm(fs::read(p).ok()?, rel).ok()
}

/// Before a 2017-18 start: keep this folder's backup beside the game if it is not there yet, or, with no backup
/// here and a game text that is not original, adopt the one beside the game ([`Beside::Foreign`] when there is
/// none or it does not match the pinned originals; nothing is changed then).
pub fn sync_beside(ed: Edition, game: &Path, backup: &Path, sheets: &Path) -> Beside {
    let c = match Ctx::new(ed, game, backup, sheets) {
        Ok(c) if ed == Edition::E1718 => c,
        _ => return Beside::Fine,
    };
    // a damaged own list counts as none: adopting the backup beside the game (checked against the originals)
    // replaces it
    let own = c.load_manifest().unwrap_or_default();
    if !own.is_empty() {
        // missing beside the game, or it lacks entries or files this folder has (a newer or bigger backup): refresh it
        let there = read_manifest(&beside(game).join("manifest.json")).unwrap_or_default();
        if own
            .keys()
            .any(|k| !there.contains_key(k) || !beside(game).join(k.replace('\\', "/")).is_file())
        {
            let _ = c.save_beside();
        }
        return Beside::Fine;
    }
    if c.text_is_original() {
        return Beside::Fine;
    }
    c.adopt_beside()
}

/// Empty folders removed, bottom up, `dir` itself included.
fn prune(dir: &Path) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                prune(&e.path());
            }
        }
    }
    let _ = fs::remove_dir(dir);
}
