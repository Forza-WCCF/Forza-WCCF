Forza-WCCF 2017-18 - 0.2.0 (alpha)
==================================
(English: README.txt - 日本語: README.ja.txt)

Alpha: si gioca, non è finita, servono tester. Leggete prima COSA NON C'È IN QUESTA ALPHA e PROBLEMI NOTI.

NOVITÀ DI QUESTA VERSIONE (non ancora pubblicata)
  - Niente Python: configurazione, catalogo delle carte, immagini delle carte e testi del gioco girano dentro
    Launcher.exe. Il download è più piccolo (niente cartelle python\ e scripts\).
  - Gioco online. Scegliete Online in Launcher.exe: la LOBBY elenca le SFIDE VS. CREA SFIDA apre una stanza con un
    codice di 4 lettere, PARTITA RAPIDA entra nella stanza aperta più vecchia (o ne apre una), i due giocatori premono
    PRONTO e un conto alla rovescia di 5 secondi avvia il gioco. Tastiera e gamepad.
  - Online ogni carta ha il proprio Aime ID: due case non condividono mai lo stesso club.
  - Le righe Server club e Collegamento VS diretto sono passate sotto ONLINE, nella sezione "Indirizzo diretto".
  - Pausa: P (o il pulsante PAUSA) ferma una partita per al massimo 20 secondi.
  - Il passo "pacchetto" dell'installer accetta lo ZIP di questa edizione (vedere il README dell'installer).

COS'È
  World Club Champion Football 2017-18 su un solo PC Windows, un solo posto, dai VOSTRI file di gioco. Il VS online con
  un'altra casa è facoltativo. Forza-WCCF sostituisce l'hardware e i server della sala giochi: server di gioco (club e
  salvataggi), rete e keychip, lettore di carte Aime, scheda I/O. Un pannello intorno alla finestra del gioco ha i
  pulsanti del cabinato e una lavagna della formazione per le carte. Forza-WCCF non contiene file originali: tutto ciò
  che viene dal gioco è creato dalla vostra copia.

COSA SERVE
  - Windows 10 o 11, a 64 bit.
  - La vostra cartella del gioco 2017-18 già decifrata (quella con client_Release.exe e control_Release.exe).
    Forza-WCCF non include il gioco. Serve la vostra copia dei file originali del gioco.
  - Nient'altro (niente WSL).

INSTALLAZIONE (una volta)
  1. Estraete questo download in una cartella tutta sua, con un percorso semplice (ad es. C:\Games\Forza-WCCF-2017-18).
  2. Avviate Launcher.exe. Windows può avvisare che il programma non è firmato: vedere PROBLEMI NOTI.
  3. Aprite "Configurazione e aggiornamenti", scegliete la cartella del gioco e premete "Configura / ripara" (circa un
     minuto). Crea una copia protetta dei programmi del gioco, il catalogo delle carte e le immagini delle carte. La
     vostra cartella non viene modificata; i programmi originali restano in original\. Rieseguendola, controlla e ripara.
  Le installazioni 2010-11 e 2017-18 sono separate: file di gioco, dati, log e salvataggi propri. Usano le stesse porte
  locali: chiudete l'una prima di avviare l'altra.

COME SI GIOCA
  Premete GIOCA in Launcher.exe. Il gioco si avvia in circa 30 secondi. La carta Aime è emulata. Club e salvataggi sono
  in data\save\gamesvr (fatene una copia). Testi del gioco: inglese, italiano o giapponese originale; sceglieteli in
  Launcher.exe prima di GIOCA.
  Una carta nuova parte con la licenza di manager, un tutorial di 5 partite e la creazione del club (circa 30 minuti).
  Per saltarli, spuntate "Salta il tutorial" in Launcher.exe prima di GIOCA: la carta riceve un club già pronto e va
  alla schermata principale.
  Fate clic sulla finestra del gioco, poi:
      Invio   START (gioco libero: la moneta viene inserita da sola)
      X       PRESS (conferma)     C   SHOOT        B   KEEPER
      S       DATA                 D   KEY PLAYER
      frecce  tattiche: Su CENTRAL, Giù COUNTER, Sinistra L-SIDE, Destra R-SIDE
      I       carta Aime dentro / fuori   5   una moneta   F1  menu di test
  oppure fate clic sui pulsanti del pannello: i triangoli (CENTRAL, COUNTER, L-SIDE, R-SIDE), PRESS, DATA, SHOOT,
  KEEPER, KEY PLAYER e START. La LAVAGNA DELLA FORMAZIONE mostra le vostre carte: trascinatene una per spostarla,
  rilasciatela su un'altra per scambiarle.
  Le carte rare con codice 2D funzionano (il server di gioco le elenca come registrate).
  Inserite la carta subito dopo l'avvio; dopo una partita toglietela (I), poi chiudete la finestra del gioco.
  P       pausa di una partita (al massimo 20 secondi, poi riprende; P o RIPRENDI riprende prima; la pausa
          successiva è possibile dopo 10 secondi). Non online, non con un'altra postazione collegata, non mentre il
          proiettore è acceso. Il tasto si cambia in KEYS.

ONLINE (VS tra due case)
  1. In Launcher.exe scegliete Online. Aprite "Indirizzo diretto" e scrivete l'indirizzo del server (HOST o
     HOST:PORTA); la lobby interroga ogni indirizzo scritto. Il server del progetto accetta solo gli indirizzi della sua
     lista consentita: chiedete a chi lo gestisce di aggiungere il vostro. Le SALE GIOCHI restano disattivate per il
     2017-18; usate SFIDE VS.
  2. CREA SFIDA apre una stanza e mostra il suo codice (ad es. K7QM). Comunicatelo all'amico, che sceglie quella
     stanza e preme ENTRA. PARTITA RAPIDA entra nella stanza aperta più vecchia, oppure ne apre una. ESCI (Esc) lascia
     il posto.
  3. Ogni giocatore preme PRONTO; un conto alla rovescia di 5 secondi avvia il gioco. Se siete pronti e nessuno entra
     entro circa 90 secondi, il gioco parte con una squadra CPU.
  4. Tasti della lobby: Su/Giù riga, Sinistra/Destra o Tab elenco, Invio entra o PRONTO, H CREA SFIDA, F5 aggiorna,
     Esc esce. Gamepad: croce direzionale o stick sinistro per muoversi, A = Invio, B = Esc, X = aggiorna,
     Y = CREA SFIDA, LB/RB = elenco.
  "Server club" (sotto Indirizzo diretto) è dove è salvato il vostro club; vuoto significa il server scelto.
  "Collegamento VS diretto" (relay.wccf.it) collega il vostro PC a quello dell'altro giocatore; togliete la spunta per
  disattivarlo. La prima volta Windows Firewall può chiedere di Launcher.exe: consentitelo per le connessioni dirette.
  Se lo negate, il VS passa dal relay, con più ritardo. Scegliete Questo PC per tornare al vostro club su questo PC.

COSA NON C'È IN QUESTA ALPHA
  - La modalità carriera.
  - Il gioco in LAN e le sale con più posti: un posto per PC. Online c'è solo il VS tra due case.
  - La scala di rendering.

PROBLEMI NOTI
  - Il programma di controllo può bloccarsi dopo circa 90 minuti di inattività. Riavviate il gioco; il club resta salvato.
  - La registrazione della squadra può attendere circa 95 secondi.
  - Il VS online tra due case non è ancora stato giocato per intero su due PC: consideratelo una prova.
  - I programmi non sono firmati. Per SmartScreen e Defender vedere ANTIVIRUS E SMARTSCREEN.
  - Gli antivirus possono segnalare overlay\inject.exe (carica il pannello nel gioco) e bin\winmm.dll (sta tra il
    gioco e Windows). Il sorgente è in source\.

ANTIVIRUS E SMARTSCREEN
  I programmi non sono firmati. Launcher.exe avvia i programmi del gioco con l'interfaccia di debug di Windows e vi
  carica due DLL di Forza-WCCF: forza_panel.dll disegna il pannello sul fotogramma del gioco; netguard.dll e wccfnet.dll
  delle copie protette sostituiscono le chiamate del gioco agli adattatori di rete, così il gioco non può far perdere a
  questo PC la concessione di rete (lease). La configurazione modifica copie dei programmi del gioco, mai gli originali
  (salvati in original\). Gli antivirus possono trattarlo come uno strumento per barare. I file di ogni versione sono
  inviati a Microsoft come falso positivo prima della pubblicazione.
  SmartScreen ("Il PC è stato protetto da Windows"): scegliete "Ulteriori informazioni", poi "Esegui comunque".
  Se Defender segnala un file: non disattivatelo. Annotate il nome del rilevamento, ripristinate il file dalla
  cronologia di protezione solo se proviene dal download ufficiale, e segnalatelo: https://github.com/Forza-WCCF/Forza-WCCF/issues

SICUREZZA
  Forza-WCCF non modifica mai le impostazioni di rete del PC (indirizzi, route, firewall). Una protezione sostituisce le
  chiamate di rete del gioco prima dell'avvio; Launcher.exe non avvia il gioco senza la protezione. Il gioco non
  raggiunge mai i server dell'editore: sono reindirizzati a sostituti su questo PC. Se Windows chiede se
  control_Release.exe può usare la rete, qualunque risposta va bene.

LOG E SEGNALAZIONI
  I log sono in data\logs; quelli dell'esecuzione precedente in data\logs\previous. Per segnalare un problema inviate i
  log e descrivete cosa avete fatto e cosa avete visto. In IMPOSTAZIONI (SETTINGS) del pannello c'è SEND LOGS (serve
  l'indirizzo di un server); altrimenti comprimete data\logs e mandatelo a mano. Per un avviso di Defender o
  SmartScreen aggiungete il nome del file e il testo dell'avviso.

CARTELLE
  bin\       winmm.dll (l'hook), FPR_Emu.exe (aiuto per il tavolo delle carte), logowin.exe (sostituto), forza-server.exe
  overlay\   forza_panel.dll (il pannello), inject.exe, netguard.dll (protezione di rete); la configurazione
             aggiunge il catalogo e le immagini delle carte
  fonts\     i caratteri del pannello e le loro licenze
  english\, italian\   i testi inglese e italiano del gioco - solo testo
  source\    il sorgente C dei programmi in bin\ e overlay\
  data\      creata sul vostro PC: impostazioni, salvataggi (save\gamesvr), log
