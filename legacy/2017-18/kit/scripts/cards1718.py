# -*- coding: utf-8 -*-
r"""cards1718.py - make the CATALOGUE's files from your own copy of WCCF 2017-18 (SDDY).  Only READS the game's files.

    python cards1718.py GAME OUT
      GAME  the folder with client_Release.exe in it (the decrypted 2017-18 "game" folder)
      OUT   where catalogue.tsv and cards\ are written (the overlay's folder)
    python cards1718.py GAME OUT --catalogue-only    catalogue.tsv only, no cards\ (setup.py does this; the Launcher
                                makes the pictures)
    python cards1718.py GAME          the self-check only: every check below, nothing written
      (needs Pillow; cards.py and ys_lzw.py are beside this file)

Writes  catalogue.tsv           one line per card: number, season, rarity, names, line, club, the six stats, total ...
                                (columns 0-30 exactly as cards.py writes them for 2010-11: the overlay reads them by
                                position; the later columns are 2017-18's own raw fields, read by nobody but people).
                                Rows run newest season first: the overlay keeps only the first 4,096 non-placeholder
                                lines (wccfpanel.c load_catalogue), so this order keeps 2012-13 .. 2017-18.
                                is_placeholder = 1 (the overlay skips it): edition 13 "not players" (referees, coaches,
                                GUESTPLAY stand-ins, the navigator) and the publisher's own joke
                                cards (NOT_PLAYERS).
        cards\<number>.png      each card's picture, 128x128 (the overlay shows its left 88 columns: the face)
Reads   client_Release.exe      card number -> record table (int32[0x2CC1] at VA 0x015B5B48; the client indexes it
                                after "card <= 0x2CC0", e.g. VA 0x453C05) and the club / nationality name table
                                (char*[286] at VA 0x0154C428: clubs 0-189, then nationalities 0-95, Japanese)
        prog_data\player_data\player_data.bin    8,434 player records of 0x620 bytes
        prog_data\player_data\VerName.txt        the edition names, one per line, line number = edition byte
        data\wccf_data.xaf                       the archive (same xaf0 format as 2010-11): data/player_card/*.dds
Exit    0 written and every check passed; 1 a check failed (nothing written if it was a catalogue check);
        2 a game file is missing or OUT cannot be written.

Club and nationality names: the game's own Japanese table (checked entry by entry below), shown in English
translated by hand (CLUBS / NATIONS: each English name sits next to the Japanese it was translated from).
"""
import os
import re
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cards as C               # the 2010-11 maker: its xaf reader, string helpers, cell(), picture writer
import ys_lzw                   # noqa: F401  (cards.write_pictures uses it; imported here so a missing file fails early)

REC = 0x620
RECORDS = 8434
CARD_TABLE_VA, CARD_TABLE_N = 0x015B5B48, 0x2CC1
NAMES_VA = 0x0154C428
PLACEHOLDER_EDITION = 13        # VerName.txt line 13 (0-based): "選手以外" (not players)
# Diego TERADONA, the publisher's own club: 3509's picture is a hand-scrawled "ダミー" (dummy), 9951 a staff Maradona
# parody
NOT_PLAYERS = {3509, 9951}
RARITY = C.RARITY               # 1 REGULAR .. 5 ALL TIME LEGEND: the 3,924 cards 2010-11 shares carry the same bytes
STAT_NAMES = C.STAT_NAMES
SEASON_FIX = {1: "2001-02", 9: "2004-05/2005-06"}       # "AOU2002", "Camp NAOMI": no years in the name (cards.py's)

COLS = ["card_no", "record_index", "set_no", "edition", "edition_label", "season", "rarity", "rarity_name",
        "rarity_frame", "is_placeholder", "name_full_latin", "name_short_latin", "name_display_ascii",
        "name_full_kana", "name_short_kana", "position_name", "club_id", "club_name", "nationality_id",
        "nationality_name", "shirt_numbers", "birth_date", "height_cm", "weight_kg"] + \
       ["stat_" + s for s in STAT_NAMES] + \
       ["stats_total", "skill_en", "skill_jp", "model_key", "image_archive_path", "image_xaf_offset",
        "image_xaf_stored_size", "image_size", "image_packed", "card_code", "club_name_jp", "nationality_name_jp",
        "u16_28e_298", "bytes_3cd_3d6", "byte_3de", "hidden_4df_523", "tail_5a4_61f"]

# club code (record +0x28C, u16) -> (the game's Japanese name, English).  "unused" entries carry no card.
CLUBS = [
    ("アタランタBC", "Atalanta"), ("ボローニャFC", "Bologna"), ("ブレシア・カルチョ", "Brescia"),
    ("キエーボ・ベローナ", "Chievo Verona"), ("ACFフィオレンティーナ", "Fiorentina"), ("FCインテル・ミラノ", "Inter"),
    ("ユベントス", "Juventus"), ("SSラツィオ", "Lazio"), ("レッチェ", "Lecce"), ("ACミラン", "AC Milan"),
    ("パルマFC", "Parma"), ("ペルージャ", "Perugia"), ("ピアチェンツァ", "Piacenza"), ("ASローマ", "AS Roma"),
    ("トリノFC", "Torino"), ("ウディネーゼ・カルチョ", "Udinese"), ("ベネツィア", "Venezia"),
    ("エラス・ベローナFC", "Hellas Verona"), ("コモ", "Como"), ("エンポリFC", "Empoli"), ("モデナ", "Modena"),
    ("レッジーナ", "Reggina"), ("UCサンプドリア", "Sampdoria"), ("バリ", "Bari"), ("カリアリ・カルチョ", "Cagliari"),
    ("ジェノアCFC", "Genoa"), ("アーセナルFC", "Arsenal"), ("チェルシーFC", "Chelsea"), ("リバプールFC", "Liverpool"),
    ("マンチェスター・ユナイテッド", "Manchester United"), ("AFCアヤックス", "Ajax"), ("フェイエノールト", "Feyenoord"),
    ("PSVアイントホーフェン", "PSV Eindhoven"), ("FCバルセロナ", "FC Barcelona"),
    ("RCデポルティボ・ラ・コルーニャ", "Deportivo La Coruna"), ("バレンシアCF", "Valencia"),
    ("ウェスト・ブロムウィッチ・アルビオンFC", "West Bromwich Albion"), ("FCメッシーナ", "Messina"),
    ("RCDマジョルカ", "Mallorca"), ("パリ・サンジェルマンFC", "Paris Saint-Germain"),
    ("ニューカッスル・ユナイテッドFC", "Newcastle United"), ("オリンピック・リヨン", "Olympique Lyonnais"),
    ("オリンピック・マルセイユ", "Olympique de Marseille"), ("FCバイエルン・ミュンヘン", "Bayern Munchen"),
    ("レアル・ベティス・バロンピエ", "Real Betis"), ("レアル・マドリーCF", "Real Madrid"), ("ビジャレアルCF", "Villarreal"),
    ("ボカ・ジュニアーズ", "Boca Juniors"), ("リーベル・プレート", "River Plate"), ("CRフラメンゴ", "Flamengo"),
    ("サントスFC", "Santos"), ("サンパウロFC", "Sao Paulo"), ("FCジロンダン・ボルドー", "Girondins de Bordeaux"),
    ("LOSCリール・メトロポール", "LOSC Lille"), ("ハンブルガーSV", "Hamburger SV"), ("ベルダー・ブレーメン", "Werder Bremen"),
    ("クラブ・アトレティコ・マドリー", "Atletico Madrid"), ("ル・マン", "Le Mans"),
    ("アイントラハト・フランクフルト", "Eintracht Frankfurt"), ("セルティックFC", "Celtic"), ("【未所属】", "(no club)"),
    ("1.FCケルン", "1. FC Koln"), ("ヤンマー", "Yanmar"), ("名古屋グランパス", "Nagoya Grampus"),
    ("カルチョ・カターニア", "Catania"), ("VVVフェンロ", "VVV-Venlo"), ("VfLボルフスブルク", "VfL Wolfsburg"),
    ("FCレッドブル・ザルツブルク", "Red Bull Salzburg"), ("SSCナポリ", "Napoli"), ("VfLボーフム", "VfL Bochum"),
    ("CSマリティモ", "Maritimo"), ("ASサンテティエンヌ", "Saint-Etienne"), ("VfBシュトゥットガルト", "VfB Stuttgart"),
    ("エバートンFC", "Everton"), ("マツダSC", "Mazda SC"), ("C.R.バスコ・ダ・ガマ", "Vasco da Gama"),
    ("AZアルクマール", "AZ Alkmaar"), ("FCポルト", "FC Porto"), ("セビージャFC", "Sevilla"), ("FC東京", "FC Tokyo"),
    ("横浜F・マリノス", "Yokohama F. Marinos"), ("鹿島アントラーズ", "Kashima Antlers"), ("ガンバ大阪", "Gamba Osaka"),
    ("CSKAモスクワ", "CSKA Moscow"), ("川崎フロンターレ", "Kawasaki Frontale"), ("グルノーブル・フット38", "Grenoble"),
    ("ヴィッセル神戸", "Vissel Kobe"), ("清水エスパルス", "Shimizu S-Pulse"), ("ジュビロ磐田", "Jubilo Iwata"),
    ("横浜FC", "Yokohama FC"), ("北海道コンサドーレ札幌", "Consadole Sapporo"), ("ガイナーレ鳥取", "Gainare Tottori"),
    ("ヴェルディ川崎", "Verdy Kawasaki"), ("セレッソ大阪", "Cerezo Osaka"), ("日産FC", "Nissan FC"),
    ("ジェフユナイテッド市原", "JEF United Ichihara"), ("ベルマーレ平塚", "Bellmare Hiratsuka"),
    ("サンフレッチェ広島", "Sanfrecce Hiroshima"), ("横浜マリノス", "Yokohama Marinos"),
    ("水原三星ブルーウィングス", "Suwon Samsung Bluewings"), ("アムカル・ペルミ", "Amkar Perm"),
    ("ボルシア・ドルトムント", "Borussia Dortmund"), ("SLベンフィカ", "Benfica"),
    ("インデペンディエンテ・メデジン", "Independiente Medellin"), ("CAベレス・サルスフィエルド", "Velez Sarsfield"),
    ("KリールセSK", "Lierse SK"), ("FCシャルケ04", "Schalke 04"), ("浦和レッズ", "Urawa Reds"),
    ("FCアウクスブルク", "FC Augsburg"), ("東京ヴェルディ1969", "Tokyo Verdy"), ("ヴァンフォーレ甲府", "Ventforet Kofu"),
    ("ジェフユナイテッド市原・千葉", "JEF United Chiba"), ("ポートランド・ティンバーズ", "Portland Timbers"),
    ("ツエーゲン金沢", "Zweigen Kanazawa"), ("横浜フリューゲルス", "Yokohama Flugels"),
    ("ロサンゼルス・ギャラクシー", "LA Galaxy"), ("リーズ・ユナイテッド", "Leeds United"),
    ("マンチェスター・シティーFC", "Manchester City"), ("レスター・シティーFC", "Leicester City"),
    ("柏レイソル", "Kashiwa Reysol"), ("アルビレックス新潟", "Albirex Niigata"), ("大宮アルディージャ", "Omiya Ardija"),
    ("愛媛FC", "Ehime FC"), ("FC" + bytes.fromhex("e382bbe382ac").decode(), "FC (the publisher)"), ("バイヤー04レバークーゼン", "Bayer Leverkusen"),
    ("オリンピアコスFC", "Olympiacos"), ("アスレティック・クラブ", "Athletic Club"),
    ("FCシャフタール・ドネツク", "Shakhtar Donetsk"), ("FCゼニト・サンクトペテルブルク", "Zenit St Petersburg"),
    ("SCインテルナシオナル", "Internacional"), ("浦項スティーラース", "Pohang Steelers"), ("フィテッセ", "Vitesse"),
    ("サウサンプトンFC", "Southampton"), ("SCコリンチャンス", "Corinthians"), ("ハノーファー96", "Hannover 96"),
    ("未使用", "(unused)"), ("VfLボルシア・メンヘングラッドバッハ", "Borussia Monchengladbach"),
    ("トッテナム・ホットスパーFC", "Tottenham Hotspur"), ("未使用2", "(unused)"), ("1FCニュルンベルク", "1. FC Nurnberg"),
    ("Rスタンダール・リエージュ", "Standard Liege"), ("未使用3", "(unused)"), ("モンペリエ・エローSC", "Montpellier"),
    ("マラガCF", "Malaga"), ("ウェストハム・ユナイテッドFC", "West Ham United"), ("未使用4", "(unused)"),
    ("フルミネンセFC", "Fluminense"), ("CFモンテレイ", "Monterrey"), ("ヘタフェCF", "Getafe"),
    ("スポルティングCP", "Sporting CP"), ("クイーンズ・パーク・レンジャーズFC", "Queens Park Rangers"),
    ("アトレチコ・ミネイロ", "Atletico Mineiro"), ("RSCアンデルレヒト", "Anderlecht"),
    ("レアル・ソシエダ・フットボル", "Real Sociedad"), ("フォルトゥナ・デュッセルドルフ1895", "Fortuna Dusseldorf"),
    ("FCユトレヒト", "FC Utrecht"), ("ASモナコFC", "AS Monaco"), ("サンダーランドAFC", "Sunderland"),
    ("ニューウェルズ・オールドボーイズ", "Newell's Old Boys"), ("スウォンジー･シティーAFC", "Swansea City"),
    ("ヘルタBSCベルリン", "Hertha BSC"), ("1FSVマインツ05", "Mainz 05"), ("TSV1860ミュンヘン", "1860 Munchen"),
    ("サガン鳥栖", "Sagan Tosu"), ("レアル・バジャドリーCF", "Real Valladolid"), ("ストーク・シティーFC", "Stoke City"),
    ("FCバーゼル1893", "FC Basel"), ("キングFC", "King FC"), ("アストン・ビラFC", "Aston Villa"),
    ("USチッタ･ディ･パレルモ", "Palermo"), ("USサッスオーロ・カルチョ", "Sassuolo"), ("FCディナモ・モスクワ", "Dynamo Moscow"),
    ("TSG1899ホッフェンハイム", "Hoffenheim"), ("ティグレスU.A.N.L.", "Tigres UANL"),
    ("ニューヨーク・シティーFC", "New York City FC"), ("クリスタル・パレスFC", "Crystal Palace"),
    ("ミドルズブラFC", "Middlesbrough"), ("湘南ベルマーレ", "Shonan Bellmare"), ("ポルティモネンセSC", "Portimonense"),
    ("BSCヤング・ボーイズ", "Young Boys"), ("アビスパ福岡", "Avispa Fukuoka"), ("徳島ヴォルティス", "Tokushima Vortis"),
    ("ファジアーノ岡山", "Fagiano Okayama"), ("FCドルトレヒト", "FC Dordrecht"), ("RCセルタ・デ・ビーゴ", "Celta Vigo"),
    ("FCメツ", "FC Metz"), ("SCヘーレンフェーン", "Heerenveen"), ("ベガルタ仙台", "Vegalta Sendai"),
    ("未使用5", "(unused)"), ("ジュニオール", "Junior"),
]
# nationality code (record +0x3CC) -> (the game's Japanese name, English)
NATIONS = [
    ("アルゼンチン", "Argentina"), ("イタリア", "Italy"), ("ウクライナ", "Ukraine"), ("オランダ", "Netherlands"),
    ("スペイン", "Spain"), ("日本", "Japan"), ("ブラジル", "Brazil"), ("ポーランド", "Poland"), ("フランス", "France"),
    ("ポルトガル", "Portugal"), ("ユーゴスラビア", "Yugoslavia"), ("チェコ", "Czech Republic"), ("デンマーク", "Denmark"),
    ("トルコ", "Turkey"), ("ウルグアイ", "Uruguay"), ("カメルーン", "Cameroon"), ("グルジア", "Georgia"), ("チリ", "Chile"),
    ("クロアチア", "Croatia"), ("ギリシャ", "Greece"), ("大韓民国", "Korea Republic"), ("イラン", "Iran"),
    ("スウェーデン", "Sweden"), ("シエラ・レオネ", "Sierra Leone"), ("ルーマニア", "Romania"), ("スロバキア", "Slovakia"),
    ("アルバニア", "Albania"), ("アイボリーコースト", "Ivory Coast"), ("オーストリア", "Austria"),
    ("コロンビア", "Colombia"), ("ガーナ", "Ghana"), ("南アフリカ", "South Africa"), ("ドイツ", "Germany"),
    ("リヒテンシュタイン", "Liechtenstein"), ("ホンジュラス", "Honduras"), ("スロベニア", "Slovenia"),
    ("オーストラリア", "Australia"), ("セネガル", "Senegal"), ("ナイジェリア", "Nigeria"), ("ベラルーシ", "Belarus"),
    ("パラグアイ", "Paraguay"), ("コスタ・リカ", "Costa Rica"), ("リトアニア", "Lithuania"),
    ("セルビア・モンテネグロ", "Serbia and Montenegro"), ("モロッコ", "Morocco"), ("リベリア", "Liberia"),
    ("エクアドル", "Ecuador"), ("ベルギー", "Belgium"), ("ロシア", "Russia"), ("パナマ", "Panama"),
    ("ボスニア・ヘルツェゴビナ", "Bosnia and Herzegovina"), ("コートジボワール", "Cote d'Ivoire"), ("エジプト", "Egypt"),
    ("イングランド", "England"), ("フィンランド", "Finland"), ("ハンガリー", "Hungary"), ("アイルランド", "Ireland"),
    ("アイスランド", "Iceland"), ("メキシコ", "Mexico"), ("ノルウェー", "Norway"), ("ペルー", "Peru"), ("スイス", "Switzerland"),
    ("チュニジア", "Tunisia"), ("アメリカ", "United States"), ("ウェールズ", "Wales"), ("北アイルランド", "Northern Ireland"),
    ("マリ", "Mali"), ("スコットランド", "Scotland"), ("カナダ", "Canada"), ("コンゴ民主共和国", "DR Congo"),
    ("ボリビア", "Bolivia"), ("アルメニア", "Armenia"), ("セルビア", "Serbia"), ("モンテネグロ", "Montenegro"),
    ("トーゴ", "Togo"), ("ギニア", "Guinea"), ("ブルガリア", "Bulgaria"), ("イスラエル", "Israel"), ("アルジェリア", "Algeria"),
    ("スリナム", "Suriname"), ("ベネズエラ", "Venezuela"), ("ブルキナファソ", "Burkina Faso"), ("マケドニア", "Macedonia"),
    ("ケニア", "Kenya"), ("イギリス", "United Kingdom"), ("プエルト・リコ", "Puerto Rico"), ("カタール", "Qatar"),
    ("ニュージーランド", "New Zealand"), ("なし", "(none)"), ("ガボン", "Gabon"), ("ジャマイカ", "Jamaica"),
    ("エストニア", "Estonia"), ("ドミニカ共和国", "Dominican Republic"), ("ウズベキスタン共和国", "Uzbekistan"),
    ("アンゴラ共和国", "Angola"), ("タイ王国", "Thailand"),
]

KNOWN = {
    7049: ("MESSI", "FW", "FC Barcelona", "Argentina", [20, 8, 20, 16, 19, 15], "1987-06-24", 170, 72),
    7064: ("RONALDO", "FW", "Real Madrid", "Portugal", [20, 5, 19, 19, 19, 15], "1985-02-05", 185, 80),
    6840: ("M.SALAH", "FW", "Liverpool", "Egypt", [16, 8, 17, 14, 19, 15], "1992-06-15", 175, 71),
    6904: ("LEWANDOWSKI", "FW", "Bayern Munchen", "Poland", [19, 6, 17, 18, 16, 16], "1988-08-21", 185, 79),
    7061: ("MODRIĆ", "MF", "Real Madrid", "Croatia", [16, 12, 19, 13, 17, 17], "1985-09-09", 174, 65),
    6883: ("KAGAWA", "MF", "Borussia Dortmund", "Japan", [16, 10, 17, 13, 16, 16], "1989-03-17", 175, 68),
    7054: ("SERGIO RAMOS", "DF", "Real Madrid", "Spain", [12, 19, 14, 18, 15, 17], "1986-03-30", 183, 75),
    7127: ("NEUER", "GK", "Bayern Munchen", "Germany", [10, 20, 14, 18, 14, 13], "1986-03-27", 193, 92),
    6954: ("BUFFON", "GK", "Juventus", "Italy", [8, 20, 13, 18, 13, 13], "1978-01-28", 191, 91),
    5980: ("BUFFON", "GK", "Juventus", "Italy", [8, 20, 13, 18, 13, 13], "1978-01-28", 192, 94),      # 2016-17
    6826: ("MIGNOLET", "GK", "Liverpool", "Belgium", [7, 18, 12, 18, 12, 11], "1988-03-06", 193, 87),
    1007: ("BA", "FW", "Chelsea", "Senegal", [19, 5, 15, 18, 17, 14], "1985-05-25", 189, 85),           # 2013-14
    4101: ("SZCZESNY", "GK", "Arsenal", "Poland", [7, 18, 12, 18, 12, 11], "1990-04-18", 195, 93),
}


def game_files(game):
    """{'exe', 'players', 'vername', 'xaf'} under GAME, or None after saying what is missing."""
    rel = {"exe": "client_Release.exe", "players": os.path.join("prog_data", "player_data", "player_data.bin"),
           "vername": os.path.join("prog_data", "player_data", "VerName.txt"),
           "xaf": os.path.join("data", "wccf_data.xaf")}
    missing = [os.path.join(game, r) for r in rel.values() if not os.path.isfile(os.path.join(game, r))]
    for m in missing:
        print("missing: %s" % m)
    return None if missing else {k: os.path.join(game, r) for k, r in rel.items()}


def exe_tables(exe):
    """({card_number: record_index}, [the 286 Japanese names: clubs 0-189, nationalities 0-95])"""
    d = open(exe, "rb").read()
    if d[:2] != b"MZ":
        raise ValueError("%s is not a Windows program" % exe)
    tab = struct.unpack_from("<%di" % CARD_TABLE_N, d, C.va_to_offset(d, CARD_TABLE_VA))
    ptrs = struct.unpack_from("<%dI" % (len(CLUBS) + len(NATIONS)), d, C.va_to_offset(d, NAMES_VA))
    names = []
    for p in ptrs:
        o = C.va_to_offset(d, p)
        names.append(d[o:d.index(b"\0", o)].decode("cp932", errors="replace"))
    return {c: i for c, i in enumerate(tab) if i >= 0}, names


def editions(path):
    """VerName.txt -> {edition: label}; each line is '<name> t|f' (t/f: the original flag, not used here)"""
    out = {}
    for i, line in enumerate(open(path, "rb").read().decode("cp932").splitlines()):
        out[i] = re.sub(r"\s+[tf]$", "", line.strip()).replace("ロケテスト", " location test").replace(
            "選手以外", "not players").replace("未使用", "unused").replace("テスト", "test")
    return out


def season_of(edition, label):
    if edition in SEASON_FIX:
        return SEASON_FIX[edition]
    m = re.search(r"(\d{4})-(\d{2})(\d{2})", label)
    return "%s-%s" % (m.group(1), m.group(3)) if m else ""


def parse(r):
    """One 0x620-byte record -> its fields.  Offsets: the 2010-11 record (cards.parse) moved to +0x288, with the
    club widened to a u16 at +0x28C and the card code added at +0x2A0 (proved against the 3,924 cards both games
    share: same names, stats, edition, rarity, line, birth date, nationality, pictures)."""
    u16 = struct.unpack_from("<11H", r, 0x288)
    return {
        "card_no": u16[0], "set_no": u16[1], "club_id": u16[2], "u16_28e_298": list(u16[3:]),
        "edition": r[0x29E], "rarity": r[0x29F], "card_code": C._cp932(r, 0x2A0, 0x10),
        "name_full_latin": C._utf16(r, 0x2B0, 0x40), "name_full_kana": C._cp932(r, 0x2F0, 0x40),
        "name_short_kana": C._cp932(r, 0x330, 0x40), "name_short_latin": C._utf16(r, 0x370, 0x40),
        "shirt_numbers": list(r[0x3B0:0x3B9]), "position": r[0x3B9], "birth_date_raw": C._ascii(r, 0x3BA, 0x10),
        "height_cm": r[0x3CA], "weight_kg": r[0x3CB], "nationality_id": r[0x3CC], "bytes_3cd_3d6": list(r[0x3CD:0x3D7]),
        "stats": list(r[0x3D7:0x3DD]), "stats_total": r[0x3DD], "byte_3de": r[0x3DE],
        "name_display_ascii": C._ascii(r, 0x3DF, 0x40), "skill_en": C._ascii(r, 0x45F, 0x40),
        "skill_jp": C._cp932(r, 0x49F, 0x40), "hidden_4df_523": list(r[0x4DF:0x524]),
        "model_key": C._ascii(r, 0x524, 0x40), "image_name": C._ascii(r, 0x564, 0x40), "tail_5a4_61f": r[0x5A4:0x620].hex(),
    }


def build_rows(files, check):
    """Every card's catalogue row, in card-number order; the checks go to check()."""
    table, names = exe_tables(files["exe"])
    d = open(files["players"], "rb").read()
    check("player_data.bin is whole 0x620-byte records", len(d) % REC == 0, "%d bytes" % len(d))
    recs = [d[i * REC:(i + 1) * REC] for i in range(len(d) // REC)]
    check("record count %d" % RECORDS, len(recs) == RECORDS, "%d records" % len(recs))
    check("the exe's table maps one card number to each record", sorted(table.values()) == list(range(len(recs))),
          "%d card numbers" % len(table))
    bad = [i for i, (jp, _) in enumerate(CLUBS + NATIONS) if names[i] != jp]
    check("the exe's club and nationality names are the ones translated here", not bad,
          "differ at %s" % bad[:10])
    eds = editions(files["vername"])
    ents = C.xaf_toc(files["xaf"])
    images = {e["name"].lower(): e for e in ents if e["is_file"] and e["path"].startswith("data/player_card/")}
    faces = {e["name"].lower() for e in ents if e["is_file"] and e["path"].startswith("data/models/face/")}
    rows, mism, exact_case, face_ok, beyond, prefix_bad = [], 0, 0, 0, 0, []
    for card_no, idx in sorted(table.items()):
        if idx >= len(recs):
            beyond += 1
            continue
        f = parse(recs[idx])
        mism += f["card_no"] != card_no
        placeholder = f["edition"] == PLACEHOLDER_EDITION or card_no in NOT_PLAYERS
        rname, rframe = RARITY.get(f["rarity"], ("", ""))
        elabel = eds.get(f["edition"], "")
        season = "" if placeholder else season_of(f["edition"], elabel)
        image = f["image_name"] or "ERROR_CARD"          # 7 placeholders name no picture: the game's error card
        img = images.get((image + ".dds").lower())
        exact_case += bool(img) and img["name"] == image + ".dds"
        face_ok += (f["model_key"] + ".svo").lower() in faces
        if f["edition"] >= 19 and season and not image.startswith(season[2:4] + season[5:7]):
            prefix_bad.append(card_no)
        club = CLUBS[f["club_id"]] if f["club_id"] < len(CLUBS) else ("", "")
        nat = NATIONS[f["nationality_id"]] if f["nationality_id"] < len(NATIONS) else ("", "")
        row = dict(f)
        row.update({
            "record_index": idx, "edition_label": elabel, "season": season, "rarity_name": rname,
            "rarity_frame": rframe, "is_placeholder": placeholder, "position_name": C.POSITIONS.get(f["position"], ""),
            "club_name": "" if placeholder else club[1], "club_name_jp": club[0],
            "nationality_name": "" if placeholder else nat[1], "nationality_name_jp": nat[0],
            "birth_date": C.birth_iso(f["birth_date_raw"]), "image_archive_path": "data/player_card/%s.dds" % image,
            "image_xaf_offset": img["offset"] if img else None, "image_xaf_stored_size": img["stored"] if img else None,
            "image_size": img["size"] if img else None, "image_packed": (img["stored"] != img["size"]) if img else None,
        })
        row.update({"stat_" + s: v for s, v in zip(STAT_NAMES, f["stats"])})
        rows.append(row)
    real = [r for r in rows if not r["is_placeholder"]]
    check("every card number in the exe's table has its record", beyond == 0, "%d point past the file's end" % beyond)
    check("the number at +0x288 of each record is its card number", mism == 0, "%d differ" % mism)
    check("every card's picture is in the archive", all(r["image_xaf_offset"] is not None for r in rows))
    check("picture names match the archive's case", exact_case == len(rows), "%d/%d" % (exact_case, len(rows)))
    check("every 2009-10+ picture name starts with its season (e.g. 1718_)", not prefix_bad, str(prefix_bad[:10]))
    # card 443 (GARGO) stores 77 while its six sum to 78 - the same in 2005-06 and 2010-11, so it is the original data
    off_sum = sorted(r["card_no"] for r in real if sum(r["stats"]) != r["stats_total"])
    check("total = the six stats added up (except the original card 443)", off_sum in ([], [443]), str(off_sum))
    check("every card's face model is in the archive", face_ok == len(rows), "%d/%d" % (face_ok, len(rows)))
    check("every position is GK, DF, MF or FW", all(r["position"] in C.POSITIONS for r in rows))
    check("every real card's rarity is 1..5", all(r["rarity"] in RARITY for r in real))
    check("every real card's edition has a season", all(r["season"] for r in real))
    check("every real card's stat is 1..20, total 60..100",
          all(1 <= v <= 20 for r in real for v in r["stats"]) and all(60 <= r["stats_total"] <= 100 for r in real))
    check("every real card's birth date reads", all(r["birth_date"] for r in real),
          "%d do not" % sum(1 for r in real if not r["birth_date"]))
    check("card numbers are unique", len({r["card_no"] for r in rows}) == len(rows))
    check("every real card's club and nationality have a name",
          all(r["club_name"] and r["nationality_name"] for r in real),
          str([r["card_no"] for r in real if not (r["club_name"] and r["nationality_name"])][:10]))
    # spot checks: short name, line, club, nationality, the six stats, birth date, height, weight - read off each
    # card's own back (data/player_card/<image>_B.dds, 256x512, 2026-10-08) except 1007/4101 (read 2026-10-07)
    by_no = {r["card_no"]: r for r in rows}
    for no, want in KNOWN.items():
        r = by_no.get(no, {})
        got = (r.get("name_short_latin"), r.get("position_name"), r.get("club_name"), r.get("nationality_name"),
               r.get("stats"), r.get("birth_date"), r.get("height_cm"), r.get("weight_kg"))
        check("card %d is %s" % (no, want[0]), got == want, "read %s" % (got,))
    check("cards 1 (referee/coach stand-in), 10570 (GUESTPLAY1) and 3509 (dummy) are placeholders",
          all(by_no.get(n, {}).get("is_placeholder") for n in (1, 10570, 3509)))
    m = by_no.get(7143, {})     # its back prints "SECRET" and no stats: the record holds 16 16 16 16 16 20
    check("card 7143 is MARADONA, ALL TIME LEGEND, stored stats 16x5 + 20",
          (m.get("name_short_latin"), m.get("rarity_name"), m.get("stats")) ==
          ("MARADONA", "ALL TIME LEGEND", [16, 16, 16, 16, 16, 20]))
    return rows, {e["path"]: e for e in ents if e["is_file"] and e["path"].startswith("data/player_card/")}


def write_catalogue(rows, path):
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8", newline="") as fh:
        fh.write("\t".join(COLS) + "\n")
        for r in sorted(rows, key=lambda r: r["season"], reverse=True):     # stable: card order within a season
            fh.write("\t".join(C.cell(r.get(c)) for c in COLS) + "\n")
    os.replace(tmp, path)                     # whole or not at all: the overlay never reads half a catalogue


def main(argv):
    try:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, ValueError):
        pass
    only = "--catalogue-only" in argv
    argv = [a for a in argv if a != "--catalogue-only"]
    if len(argv) not in (1, 2):
        print(__doc__)
        return 2
    game = os.path.abspath(argv[0])
    files = game_files(game)
    if not files:
        print("GAME must be the 2017-18 folder that holds client_Release.exe")
        return 2
    print("game files: %s" % os.path.dirname(files["exe"]))
    checks = []
    def check(name, ok, detail=""):
        checks.append((name, bool(ok), detail))
    try:
        rows, toc = build_rows(files, check)
    except (ValueError, struct.error, OSError, IndexError) as ex:
        print("FAIL  reading the game's files: %s" % ex)
        return 1
    for name, ok, detail in checks:
        print("  %s  %s%s" % ("PASS" if ok else "FAIL", name, ("  - " + detail) if detail and not ok else ""))
    if not all(ok for _, ok, _ in checks):
        print("not written: this copy does not match WCCF 2017-18's player data")
        return 1
    if len(argv) == 1:                        # the self-check: read and checked, nothing to write
        return 0

    cards_dir = os.path.join(os.path.abspath(argv[1]), "cards")
    if only:
        os.makedirs(os.path.dirname(cards_dir), exist_ok=True)
        write_catalogue(rows, os.path.join(os.path.dirname(cards_dir), "catalogue.tsv"))
        print("catalogue.tsv: %d cards" % len(rows))
        return 0
    if os.path.islink(cards_dir) or (hasattr(os.path, "isjunction") and os.path.isjunction(cards_dir)):
        print("refused: %s is a link to another folder - writing would change that folder" % cards_dir)
        return 2
    try:
        os.makedirs(cards_dir, exist_ok=True)
    except OSError as ex:
        print("cannot make %s: %s" % (cards_dir, ex))
        return 2
    write_catalogue(rows, os.path.join(os.path.dirname(cards_dir), "catalogue.tsv"))
    print("catalogue.tsv: %d cards (%d real, %d placeholders)" % (
        len(rows), sum(1 for r in rows if not r["is_placeholder"]), sum(1 for r in rows if r["is_placeholder"])))
    done, fails = C.write_pictures(rows, toc, files["xaf"], cards_dir)
    print("pictures: %d of %d written" % (done, len(rows)))
    for msg in fails[:20]:
        print("  FAIL  " + msg)
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
