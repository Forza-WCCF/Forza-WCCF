Forza-WCCF 2017-18 - 0.1.0-alpha.1
==================================
(English: README.txt - 日本語: README.ja.txt)

Questa è una alpha: si gioca, ma non è finita e servono tester. Prima di iniziare leggete COSA NON C'È IN QUESTA ALPHA
e PROBLEMI NOTI.

COS'È
  World Club Champion Football 2017-18 su un solo PC Windows, offline, un solo posto, dai VOSTRI file di gioco.
  Forza-WCCF sostituisce l'hardware e i server della sala giochi: il server di gioco (il club e i suoi salvataggi), la
  rete e il keychip, il lettore di carte Aime, la scheda I/O. Intorno alla finestra del gioco c'è un pannello con i
  pulsanti del cabinato e una lavagna della formazione per le carte. Forza-WCCF non contiene nessun file originale:
  tutto ciò che viene dal gioco è creato dalla vostra copia.

COSA SERVE
  - Windows 10 o 11, a 64 bit.
  - La vostra cartella del gioco 2017-18 già decifrata (quella con client_Release.exe e control_Release.exe).
    <dove trovare il gioco: da compilare a cura del proprietario>
  - Nient'altro: niente WSL, niente installazione di Python (Python è incluso).

INSTALLAZIONE (una volta)
  1. Estraete questo download in una cartella tutta sua, con un percorso semplice (ad es. C:\Games\Forza-WCCF-2017-18).
  2. Avviate Launcher.exe. Windows può avvisare che il programma non è firmato: vedere PROBLEMI NOTI.
  3. Aprite "Configurazione e aggiornamenti", scegliete la cartella del gioco e premete "Configura / ripara". La
     configurazione crea una copia protetta dei programmi del gioco (la cartella originale non viene modificata), il
     catalogo delle carte e le immagini delle carte; richiede circa un minuto. Rieseguendola, controlla e ripara.
  Le installazioni 2010-11 e 2017-18 restano separate, ciascuna con i propri file di gioco, dati, log e salvataggi.
  Non possono funzionare insieme sullo stesso PC: usano le stesse porte locali. Chiudete l'una prima di avviare l'altra.

COME SI GIOCA
  Premete GIOCA in Launcher.exe. Il gioco impiega circa 30 secondi ad avviarsi. La carta Aime è emulata; il club e i
  suoi salvataggi restano su questo PC, in data\save\gamesvr (fatene una copia di sicurezza). Testi del gioco: inglese,
  italiano o giapponese originale - sceglieteli in Launcher.exe prima di GIOCA.
  La prima volta: una carta nuova parte con la licenza di manager, un tutorial di 5 partite e la creazione del club,
  circa 30 minuti. Nella configurazione potete attivare "Salta il tutorial": una carta nuova riceve un club già pronto
  e va direttamente alla schermata principale.
  Fate clic sulla finestra del gioco, poi:
      Invio   START (gioco libero: la moneta viene inserita da sola)
      X       PRESS (conferma)     C   SHOOT        B   KEEPER
      S       DATA                 D   KEY PLAYER
      frecce  tattiche: Su CENTRAL, Giù COUNTER, Sinistra L-SIDE, Destra R-SIDE
      I       carta Aime dentro / fuori   5   una moneta   F1  menu di test
  oppure fate clic sui pulsanti del pannello: i triangoli (CENTRAL, COUNTER, L-SIDE, R-SIDE), PRESS, DATA, SHOOT,
  KEEPER, KEY PLAYER e START. La LAVAGNA DELLA FORMAZIONE mostra le vostre carte: trascinatene una per spostarla,
  rilasciatela su un'altra per scambiarle.
  Le carte rare con codice 2D funzionano: il server di gioco le elenca come registrate.
  Inserite la carta subito dopo l'avvio; dopo una partita toglietela (I), poi chiudete la finestra del gioco.

COSA NON C'È IN QUESTA ALPHA
  - La modalità carriera.
  - Il gioco online o in LAN: un PC, un posto.
  - Il supporto dell'installer per il 2017-18: si installa estraendo lo ZIP, come sopra.
  - La pausa e la scala di rendering.

PROBLEMI NOTI
  - Il programma di controllo può bloccarsi dopo circa 90 minuti di inattività. Chiudete il gioco e riavviatelo; il
    club resta salvato.
  - La registrazione della squadra può attendere circa 95 secondi prima di proseguire.
  - I programmi non sono firmati. Windows SmartScreen può mostrare "Il PC è stato protetto da Windows": scegliete
    "Ulteriori informazioni", poi "Esegui comunque". Se Microsoft Defender segnala un file, non disattivate Defender:
    segnalate il nome del file e il messaggio di Defender (vedere LOG E SEGNALAZIONI).
  - Gli antivirus possono non gradire overlay\inject.exe (carica il pannello nel gioco) e bin\winmm.dll (sta tra il
    gioco e Windows). Il loro codice sorgente è in source\.

SICUREZZA
  Forza-WCCF non modifica mai le impostazioni di rete del PC (indirizzi, route, firewall). Le chiamate di rete del
  gioco vengono sostituite da una protezione prima dell'avvio, e Launcher.exe rifiuta di avviare il gioco se la
  protezione non è a posto. Il gioco non raggiunge mai i server dell'editore: sono reindirizzati a sostituti su questo
  PC. Windows può chiedere se control_Release.exe può usare la rete: qualunque risposta va bene, tutto resta su questo PC.

LOG E SEGNALAZIONI
  I log sono in data\logs; quelli dell'esecuzione precedente in data\logs\previous. Per segnalare un problema inviate i
  log: in IMPOSTAZIONI (SETTINGS) del pannello c'è SEND LOGS (serve l'indirizzo di un server a cui inviarli; altrimenti
  comprimete data\logs e mandatelo a mano), e descrivete cosa avete fatto e cosa avete visto. Per un avviso di Defender
  o SmartScreen aggiungete il nome del file e il testo dell'avviso.

CARTELLE
  bin\       winmm.dll (l'hook), FPR_Emu.exe (aiuto per il tavolo delle carte), logowin.exe (sostituto), forza-server.exe
  overlay\   forza_panel.dll (il pannello), inject.exe, netguard.dll (protezione di rete), skin.tex; la configurazione
             aggiunge il catalogo e le immagini delle carte
  fonts\     i caratteri del pannello e le loro licenze
  scripts\   configurazione, creazione delle carte e strumenti per i testi inglese e italiano
  english\, italian\   i testi inglese e italiano del gioco - solo testo
  source\    il sorgente C dei programmi in bin\ e overlay\
  python\    Python 3.13 con Pillow
  data\      creata sul vostro PC: impostazioni, salvataggi (save\gamesvr), log
