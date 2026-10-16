//! Launcher.exe's words: English, Italian or Japanese - the one picked in the window (data\settings.json "language"),
//! else Windows' display language.  The kit's scripts (their output in the window) stay in English.

use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;

pub struct Text {
    pub title: &'static str,
    pub play: &'static str,
    pub stop_game: &'static str,
    pub stop_confirm: &'static str,
    pub play_on: &'static str,
    pub this_pc: &'static str,
    pub online: &'static str,
    pub language: &'static str,
    pub more: &'static str,
    pub less: &'static str,
    pub starting: &'static str,
    pub restarting: &'static str,
    pub stopping: &'static str,
    pub running: &'static str,
    pub not_set_up: &'static str,
    pub bad_address: &'static str,
    pub start_failed: &'static str,
    pub game_folder: &'static str,
    pub browse: &'static str,
    pub browse_title: &'static str,
    pub set_up: &'static str,
    pub undo: &'static str,
    pub undo_confirm: &'static str,
    pub no_folder: &'static str,
    pub game_text: &'static str,
    pub english: &'static str,
    pub italian: &'static str,
    pub japanese: &'static str,
    pub update_heading: &'static str,
    pub not_known: &'static str,
    pub update: &'static str,
    pub whats_new: &'static str,
    pub release: &'static str,
    pub test_build: &'static str,
    pub this_kit: &'static str,
    pub newer: &'static str,
    pub available: &'static str,
    pub asking: &'static str,
    pub pick: &'static str,
    pub none_listed: &'static str,
    pub no_answer: &'static str,
    pub retry: &'static str,
    pub confirm_release: &'static str,
    pub confirm_test: &'static str,
    pub downloading: &'static str,
    pub unzipping: &'static str,
    pub copying: &'static str,
    pub updated: &'static str,
    pub after_update: &'static str,
    pub after_update_english: &'static str,
    pub after_update_italian: &'static str,
    pub close_game: &'static str,
    pub download_failed: &'static str,
    pub unzip_failed: &'static str,
    pub not_a_kit: &'static str,
    pub write_failed: &'static str,
    pub done: &'static str,
    pub failed: &'static str,
    pub busy: &'static str,
    pub unguarded: &'static str,
    pub not_guarded: &'static str,
    pub setup_1718: &'static str,
    pub stop_anyway: &'static str,
    pub link_taken: &'static str,
    pub link_no_junction: &'static str,
    pub link_failed: &'static str,
    pub card_pictures: &'static str,
    pub from_game: &'static str,
    pub hq_pack: &'static str,
    pub choose_pack: &'static str,
    pub pack_title: &'static str,
    pub pictures_next: &'static str,
}

const EN: Text = Text {
    play: "PLAY",
    stop_game: "STOP THE GAME",
    stop_confirm: "Stop the game now?\n\nIf you can, close its window instead. During a match, stopping it gives your card a bad \
                   ending (trade rights are lost at 2).",
    play_on: "Play on:",
    this_pc: "This PC",
    online: "Online, server:",
    language: "Language:",
    more: "Setup and updates  ▼",
    less: "Setup and updates  ▲",
    starting: "Starting the game - about 30 seconds ...",
    restarting: "Restarting, as SETTINGS asked ...",
    stopping: "Stopping the game ...",
    running: "The game is running. Close its window to quit (STOP THE GAME if it is stuck).",
    not_set_up: "Set up first: pick your game folder under Setup and updates, then Set up / repair.",
    bad_address: "Type the server's address: four numbers with dots, e.g. 192.168.1.20.",
    start_failed: "The game did not start - the details below say why.",
    title: "WCCF 2010-11 kit",
    game_folder: "Game folder:",
    browse: "Browse...",
    browse_title: "Pick the game folder (the one with client_Release.exe)",
    set_up: "Set up / repair",
    undo: "Undo setup",
    undo_confirm: "Put the game folder back the way it was before setup?",
    no_folder: "Pick the game folder first (the one with client_Release.exe).",
    game_text: "Game text:",
    english: "English",
    italian: "Italian",
    japanese: "Japanese",
    update_heading: "Update the kit - this kit: {}",
    not_known: "not known",
    update: "UPDATE",
    whats_new: "What's new",
    release: "release",
    test_build: "test build",
    this_kit: "(this kit)",
    newer: "(newer)",
    available: "A newer kit is out: {} - Setup and updates, UPDATE.",
    asking: "Looking for updates ...",
    pick: "Pick a kit, then UPDATE. \"What's new\" opens its page.",
    none_listed: "No update found.",
    no_answer: "The update server did not answer: {}",
    retry: "Close Launcher and open it again to retry.",
    confirm_release: "Put the release {} into this kit folder?\n\nYour club cards, keys and settings (data) are kept. \
                      Close the game first.",
    confirm_test: "Put the test build {} into this kit folder?\n\nYour club cards, keys and settings (data) are kept. \
                   Close the game first.",
    downloading: "Downloading {} ...",
    unzipping: "Unzipping ...",
    copying: "Copying {} into the kit folder ...",
    updated: "This kit folder now holds {}.",
    after_update: "The kit is updated.\n\nSet up the game folder for the new kit now?",
    after_update_english: "The kit is updated.\n\nSet up the game folder for the new kit now, and put its English in?",
    after_update_italian: "The kit is updated.\n\nSet up the game folder for the new kit now, and put its Italian in?",
    close_game: "Close the game first: {} runs from the kit folder.",
    download_failed: "The download failed: {}",
    unzip_failed: "The ZIP did not unzip: {}",
    not_a_kit: "This ZIP is not a kit: no folder in it holds Launcher.exe.",
    write_failed: "{}: could not write it ({}) - close the game and update again.",
    done: "Done.",
    failed: "Failed - the output below says why.",
    busy: "Wait: Launcher is busy.",
    unguarded: "NOT started: Windows was told to load only its own copies of system files for programs started from \
                here (\"Prefer System32 images\" - a WSL terminal does this), so the game would run without the kit's \
                guard. Start Launcher.exe from Explorer (double-click it).",
    not_guarded: "NOT started: the 2017-18 network guard is not in place (setup-hashes.json is missing or does not \
                  match, or a game program is not the guarded copy), so the game could change this PC's network \
                  settings. Run setup1718.py again (legacy/2017-18/tools/setup1718.py), then start again. The details \
                  are in Launcher's window.",
    setup_1718: "The 2017-18 game is set up with setup1718.py (legacy/2017-18/tools/setup1718.py, from WSL), not here.",
    stop_anyway: "Your club card is in the reader, or a match is on: stopping now can give the card a bad ending \
                  (trade rights are lost at 2). The details are in Launcher's window.\n\nStop the game anyway?",
    link_taken: "This folder's path has special characters, which the game may not handle, and {} is already used by another Forza-WCCF folder. Running from here. If the game does not start, move the kit to a plain path such as C:\\Games.",
    link_no_junction: "This folder's path has special characters, which the game may not handle, and Windows can only link folders on local NTFS drives. Running from here. If the game does not start, move the kit to a plain path such as C:\\Games.",
    link_failed: "This folder's path has special characters, which the game may not handle, and linking it to {} failed ({}). Running from here. If the game does not start, move the kit to a plain path such as C:\\Games.",
    card_pictures: "Card pictures:",
    from_game: "From the game",
    hq_pack: "High-quality pack",
    choose_pack: "Choose pack...",
    pack_title: "Pick the high-quality card pack (forza-wccf-cards-*.zip)",
    pictures_next: "The card pictures change at the next PLAY (about a minute the first time).",
};

const IT: Text = Text {
    play: "GIOCA",
    stop_game: "FERMA IL GIOCO",
    stop_confirm: "Fermare il gioco adesso?\n\nSe puoi, chiudi invece la sua finestra. Durante una partita, fermarlo dà alla tua \
                   carta un bad ending (a 2 si perdono i diritti di scambio).",
    play_on: "Gioca su:",
    this_pc: "Questo PC",
    online: "Online, server:",
    language: "Lingua:",
    more: "Configurazione e aggiornamenti  ▼",
    less: "Configurazione e aggiornamenti  ▲",
    starting: "Avvio del gioco - circa 30 secondi ...",
    restarting: "Riavvio, come richiesto da SETTINGS ...",
    stopping: "Arresto del gioco ...",
    running: "Il gioco è in esecuzione. Chiudi la sua finestra per uscire (FERMA IL GIOCO se è bloccato).",
    not_set_up: "Prima la configurazione: scegli la cartella del gioco in Configurazione e aggiornamenti, poi Configura / ripara.",
    bad_address: "Scrivi l'indirizzo del server: quattro numeri separati da punti, ad es. 192.168.1.20.",
    start_failed: "Il gioco non è partito - il motivo è nei dettagli qui sotto.",
    title: "Kit WCCF 2010-11",
    game_folder: "Cartella del gioco:",
    browse: "Sfoglia...",
    browse_title: "Scegli la cartella del gioco (quella con client_Release.exe)",
    set_up: "Configura / ripara",
    undo: "Annulla configurazione",
    undo_confirm: "Riportare la cartella del gioco com'era prima della configurazione?",
    no_folder: "Scegli prima la cartella del gioco (quella con client_Release.exe).",
    game_text: "Testi del gioco:",
    english: "Inglese",
    italian: "Italiano",
    japanese: "Giapponese",
    update_heading: "Aggiorna il kit - questo kit: {}",
    not_known: "sconosciuto",
    update: "AGGIORNA",
    whats_new: "Novità",
    release: "versione",
    test_build: "prova",
    this_kit: "(questo kit)",
    newer: "(più recente)",
    available: "È uscito un kit più recente: {} - Configurazione e aggiornamenti, AGGIORNA.",
    asking: "Ricerca degli aggiornamenti ...",
    pick: "Scegli un kit, poi AGGIORNA. \"Novità\" apre la sua pagina.",
    none_listed: "Nessun aggiornamento trovato.",
    no_answer: "Il server degli aggiornamenti non ha risposto: {}",
    retry: "Chiudi Launcher e riaprilo per riprovare.",
    confirm_release: "Mettere la versione {} in questa cartella del kit?\n\nLe tue carte club, i tasti e le \
                      impostazioni (data) restano. Chiudi prima il gioco.",
    confirm_test: "Mettere la versione di prova {} in questa cartella del kit?\n\nLe tue carte club, i tasti e le \
                   impostazioni (data) restano. Chiudi prima il gioco.",
    downloading: "Download di {} ...",
    unzipping: "Estrazione ...",
    copying: "Copia di {} nella cartella del kit ...",
    updated: "Questa cartella del kit ora contiene {}.",
    after_update: "Il kit è aggiornato.\n\nConfigurare ora la cartella del gioco per il nuovo kit?",
    after_update_english: "Il kit è aggiornato.\n\nConfigurare ora la cartella del gioco per il nuovo kit e rimettere \
                           l'inglese?",
    after_update_italian: "Il kit è aggiornato.\n\nConfigurare ora la cartella del gioco per il nuovo kit e rimettere \
                           l'italiano?",
    close_game: "Chiudi prima il gioco: {} è in esecuzione dalla cartella del kit.",
    download_failed: "Download non riuscito: {}",
    unzip_failed: "Impossibile estrarre lo ZIP: {}",
    not_a_kit: "Questo ZIP non è un kit: nessuna sua cartella contiene Launcher.exe.",
    write_failed: "{}: scrittura non riuscita ({}) - chiudi il gioco e aggiorna di nuovo.",
    done: "Fatto.",
    failed: "Non riuscito - il motivo è nel testo qui sotto.",
    busy: "Attendi: Launcher è occupato.",
    unguarded: "NON avviato: Windows ha ricevuto l'ordine di caricare solo le proprie copie dei file di sistema per i \
                programmi avviati da qui (\"Prefer System32 images\" - lo fa un terminale WSL), quindi il gioco \
                girerebbe senza la protezione del kit. Avvia Launcher.exe da Esplora file (doppio clic).",
    not_guarded: "NON avviato: la protezione di rete del 2017-18 non è a posto (setup-hashes.json manca o non \
                  corrisponde, oppure un programma del gioco non è la copia protetta), quindi il gioco potrebbe \
                  cambiare le impostazioni di rete di questo PC. Esegui di nuovo setup1718.py \
                  (legacy/2017-18/tools/setup1718.py), poi riavvia. I dettagli sono nella finestra di Launcher.",
    setup_1718: "Il gioco 2017-18 si configura con setup1718.py (legacy/2017-18/tools/setup1718.py, da WSL), non qui.",
    stop_anyway: "La tua carta del club è nel lettore, oppure è in corso una partita: fermare adesso può dare alla \
                  carta un finale negativo (a 2 si perdono i diritti di scambio). I dettagli sono nella finestra di \
                  Launcher.\n\nFermare comunque il gioco?",
    link_taken: "Il percorso di questa cartella ha caratteri speciali che il gioco potrebbe non gestire, e {} è già usata da un'altra cartella Forza-WCCF. Avvio da qui. Se il gioco non parte, sposta il kit in un percorso semplice come C:\\Games.",
    link_no_junction: "Il percorso di questa cartella ha caratteri speciali che il gioco potrebbe non gestire, e Windows può collegare solo cartelle su dischi NTFS locali. Avvio da qui. Se il gioco non parte, sposta il kit in un percorso semplice come C:\\Games.",
    link_failed: "Il percorso di questa cartella ha caratteri speciali che il gioco potrebbe non gestire, e il collegamento a {} non è riuscito ({}). Avvio da qui. Se il gioco non parte, sposta il kit in un percorso semplice come C:\\Games.",
    card_pictures: "Immagini carte:",
    from_game: "Dal gioco",
    hq_pack: "Pacchetto alta qualità",
    choose_pack: "Scegli pacchetto...",
    pack_title: "Scegli il pacchetto di immagini carte in alta qualità (forza-wccf-cards-*.zip)",
    pictures_next: "Le immagini delle carte cambiano al prossimo GIOCA (circa un minuto la prima volta).",
};

const JA: Text = Text {
    play: "プレイ",
    stop_game: "ゲームを終了",
    stop_confirm: "今すぐゲームを終了しますか？\n\nできれば代わりにゲームのウィンドウを閉じてください。試合中に終了すると、カードに\
                   バッドエンドが記録されます（2 回でトレード権を失います）。",
    play_on: "プレイ先:",
    this_pc: "この PC",
    online: "オンライン:",
    language: "言語:",
    more: "セットアップと更新  ▼",
    less: "セットアップと更新  ▲",
    starting: "ゲームを起動しています（約 30 秒）...",
    restarting: "SETTINGS の指示で再起動しています...",
    stopping: "ゲームを終了しています...",
    running: "ゲームは実行中です。終了するにはゲームのウィンドウを閉じてください（固まった場合は「ゲームを終了」）。",
    not_set_up: "まずセットアップしてください:「セットアップと更新」でゲームフォルダーを選び、「セットアップ / 修復」を押します。",
    bad_address: "サーバーのアドレスを入力してください（ドットで区切った 4 つの数字、例: 192.168.1.20）。",
    start_failed: "ゲームを起動できませんでした（理由は下の詳細にあります）。",
    title: "WCCF 2010-11 キット",
    game_folder: "ゲームフォルダー:",
    browse: "参照...",
    browse_title: "ゲームフォルダー（client_Release.exe があるフォルダー）を選んでください",
    set_up: "セットアップ / 修復",
    undo: "セットアップを元に戻す",
    undo_confirm: "ゲームフォルダーをセットアップ前の状態に戻しますか？",
    no_folder: "先にゲームフォルダー（client_Release.exe があるフォルダー）を選んでください。",
    game_text: "ゲームの文字:",
    english: "英語",
    italian: "イタリア語",
    japanese: "日本語",
    update_heading: "キットの更新 - このキット: {}",
    not_known: "不明",
    update: "更新",
    whats_new: "変更内容",
    release: "リリース",
    test_build: "テスト版",
    this_kit: "（このキット）",
    newer: "（新しい版）",
    available: "新しいキットが公開されています: {}（「セットアップと更新」の「更新」）",
    asking: "更新を確認しています...",
    pick: "キットを選んで「更新」を押してください。「変更内容」でそのページを開きます。",
    none_listed: "更新は見つかりませんでした。",
    no_answer: "更新サーバーから応答がありません: {}",
    retry: "Launcher を閉じて、もう一度開いてください。",
    confirm_release: "リリース {} をこのキットフォルダーに入れますか？\n\nクラブカード、キー、設定（data）はそのまま残ります。\
                      先にゲームを閉じてください。",
    confirm_test: "テスト版 {} をこのキットフォルダーに入れますか？\n\nクラブカード、キー、設定（data）はそのまま残ります。\
                   先にゲームを閉じてください。",
    downloading: "{} をダウンロードしています...",
    unzipping: "展開しています...",
    copying: "{} をキットフォルダーにコピーしています...",
    updated: "このキットフォルダーは {} になりました。",
    after_update: "キットを更新しました。\n\n新しいキット用にゲームフォルダーをセットアップしますか？",
    after_update_english: "キットを更新しました。\n\n新しいキット用にゲームフォルダーをセットアップし、英語化もやり直しますか？",
    after_update_italian: "キットを更新しました。\n\n新しいキット用にゲームフォルダーをセットアップし、イタリア語化もやり直しますか？",
    close_game: "先にゲームを閉じてください: {} がキットフォルダーから実行されています。",
    download_failed: "ダウンロードに失敗しました: {}",
    unzip_failed: "ZIP を展開できませんでした: {}",
    not_a_kit: "この ZIP はキットではありません（Launcher.exe を含むフォルダーがありません）。",
    write_failed: "{} に書き込めませんでした（{}）。ゲームを閉じて、もう一度更新してください。",
    done: "完了しました。",
    failed: "失敗しました（理由は下の表示にあります）。",
    busy: "処理中です。終わるまでお待ちください。",
    unguarded: "起動しませんでした: ここから起動するプログラムは Windows 自身のシステムファイルだけを読み込むよう指示されて\
                います（\"Prefer System32 images\" - WSL のターミナルがこれを行います）。このままではキットの保護なしで\
                ゲームが動きます。エクスプローラーから Launcher.exe を起動してください（ダブルクリック）。",
    not_guarded: "起動しませんでした: 2017-18 のネットワーク保護が整っていません（setup-hashes.json がないか一致しない、\
                  またはゲームのプログラムが保護済みのコピーではありません）。このままではゲームがこの PC のネットワーク\
                  設定を変えるおそれがあります。setup1718.py（legacy/2017-18/tools/setup1718.py）をもう一度実行してから\
                  起動してください。詳細は Launcher のウィンドウにあります。",
    setup_1718: "2017-18 のゲームはここではなく setup1718.py（legacy/2017-18/tools/setup1718.py、WSL から）で設定します。",
    stop_anyway: "クラブカードがリーダーに入っているか、試合中です。今終了するとカードにバッドエンドが付くことがあります\
                  （2 回でトレード権を失います）。詳細は Launcher のウィンドウにあります。\n\nそれでもゲームを終了しますか？",
    link_taken: "このフォルダーのパスには、ゲームが扱えないかもしれない特殊な文字があり、{} は別の Forza-WCCF フォルダーが使用中です。ここから起動します。ゲームが起動しない場合は、キットを C:\\Games のような英数字だけのパスへ移動してください。",
    link_no_junction: "このフォルダーのパスには、ゲームが扱えないかもしれない特殊な文字があり、Windows がリンクできるのはローカルの NTFS ドライブ上のフォルダーだけです。ここから起動します。ゲームが起動しない場合は、キットを C:\\Games のような英数字だけのパスへ移動してください。",
    link_failed: "このフォルダーのパスには、ゲームが扱えないかもしれない特殊な文字があり、{} へのリンクに失敗しました（{}）。ここから起動します。ゲームが起動しない場合は、キットを C:\\Games のような英数字だけのパスへ移動してください。",
    card_pictures: "カード画像:",
    from_game: "ゲームから",
    hq_pack: "高画質パック",
    choose_pack: "パックを選択...",
    pack_title: "高画質カード画像パックを選んでください（forza-wccf-cards-*.zip）",
    pictures_next: "カード画像は次の「プレイ」から変わります（初回は約1分かかります）。",
};

/// The languages, in the window's list: code, name in its own language, words.
pub const LANGUAGES: [(&str, &str, &Text); 3] =
    [("en", "English", &EN), ("it", "Italiano", &IT), ("ja", "日本語", &JA)];

static CHOSEN: AtomicUsize = AtomicUsize::new(usize::MAX);

/// The words picked: `set` (the window's list), else WCCF_KIT_LANG=en|it|ja (to check a translation on any PC), the
/// one saved in data\settings.json, Windows' display language (LANG_ITALIAN 0x10, LANG_JAPANESE 0x11), English.
pub fn choose(saved: Option<&str>) {
    let env = std::env::var("WCCF_KIT_LANG").ok();
    // SAFETY: a plain query without arguments.
    let windows = match unsafe { GetUserDefaultUILanguage() } & 0x3ff {
        0x10 => "it",
        0x11 => "ja",
        _ => "en",
    };
    let code = [env.as_deref(), saved]
        .into_iter()
        .flatten()
        .find(|c| index(c).is_some())
        .unwrap_or(windows);
    set(code);
}

fn index(code: &str) -> Option<usize> {
    LANGUAGES.iter().position(|(c, _, _)| *c == code)
}

pub fn set(code: &str) {
    CHOSEN.store(index(code).unwrap_or(0), Ordering::Relaxed);
}

/// The chosen language's place in LANGUAGES.
pub fn chosen() -> usize {
    CHOSEN.load(Ordering::Relaxed).min(LANGUAGES.len() - 1)
}

pub fn t() -> &'static Text {
    LANGUAGES[chosen()].2
}

/// `text` with each "{}" filled, in order.
pub fn fill(text: &str, args: &[&str]) -> String {
    let mut out = String::new();
    let mut parts = text.split("{}");
    out.push_str(parts.next().unwrap_or(""));
    for (i, part) in parts.enumerate() {
        out.push_str(args.get(i).copied().unwrap_or(""));
        out.push_str(part);
    }
    out
}
