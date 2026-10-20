Forza-WCCF 2017-18 - 0.1.0-alpha.1
==================================
(English: README.txt - 日本語: README.ja.txt)

Questa è una alpha: si gioca, ma non è finita e servono tester. Prima di iniziare leggete COSA NON C'È IN QUESTA ALPHA
e PROBLEMI NOTI.

NOVITÀ DI QUESTA VERSIONE (non ancora pubblicata)
  - Niente Python: configurazione, catalogo delle carte, immagini delle carte e testi del gioco girano dentro
    Launcher.exe. Non c'è nulla da installare; il download è più piccolo (niente cartelle python\ e scripts\).
  - Il gioco online è attivo. Scegliete Online in Launcher.exe: la LOBBY elenca le SFIDE VS. CREA SFIDA apre una
    stanza con un codice di 4 lettere, PARTITA RAPIDA entra nella stanza aperta più vecchia (o ne apre una), i due
    giocatori premono PRONTO e un conto alla rovescia di 5 secondi avvia il gioco. La lobby si usa con la tastiera e
    con un gamepad.
  - Online ogni carta ha il proprio Aime ID, così due case non condividono mai lo stesso club.
  - Le righe Server club e Collegamento VS diretto sono passate sotto ONLINE, nella sezione "Indirizzo diretto".
  - Pausa: P (o il pulsante PAUSA) ferma una partita su questo PC per al massimo 20 secondi.
  - Il passo --kit dell'installer accetta anche lo ZIP di questa edizione (vedere il README dell'installer).

COS'È
  World Club Champion Football 2017-18 su un solo PC Windows, un solo posto, dai VOSTRI file di gioco; il VS online con
  un'altra casa è facoltativo.
  Forza-WCCF sostituisce l'hardware e i server della sala giochi: il server di gioco (il club e i suoi salvataggi), la
  rete e il keychip, il lettore di carte Aime, la scheda I/O. Intorno alla finestra del gioco c'è un pannello con i
  pulsanti del cabinato e una lavagna della formazione per le carte. Forza-WCCF non contiene nessun file originale:
  tutto ciò che viene dal gioco è creato dalla vostra copia.

COSA SERVE
  - Windows 10 o 11, a 64 bit.
  - La vostra cartella del gioco 2017-18 già decifrata (quella con client_Release.exe e control_Release.exe).
    <dove trovare il gioco: da compilare a cura del proprietario>
  - Nient'altro: niente WSL, niente da installare.

INSTALLAZIONE (una volta)
  1. Estraete questo download in una cartella tutta sua, con un percorso semplice (ad es. C:\Games\Forza-WCCF-2017-18).
  2. Avviate Launcher.exe. Windows può avvisare che il programma non è firmato: vedere PROBLEMI NOTI.
  3. Aprite "Configurazione e aggiornamenti", scegliete la cartella del gioco e premete "Configura / ripara". La
     configurazione crea una copia protetta dei programmi del gioco (la cartella originale non viene modificata; i programmi originali restano in original\), il
     catalogo delle carte e le immagini delle carte; richiede circa un minuto. Rieseguendola, controlla e ripara.
  Le installazioni 2010-11 e 2017-18 restano separate, ciascuna con i propri file di gioco, dati, log e salvataggi.
  Non possono funzionare insieme sullo stesso PC: usano le stesse porte locali. Chiudete l'una prima di avviare l'altra.

COME SI GIOCA
  Premete GIOCA in Launcher.exe. Il gioco impiega circa 30 secondi ad avviarsi. La carta Aime è emulata; il club e i
  suoi salvataggi restano su questo PC, in data\save\gamesvr (fatene una copia di sicurezza). Testi del gioco: inglese,
  italiano o giapponese originale - sceglieteli in Launcher.exe prima di GIOCA.
  La prima volta: una carta nuova parte con la licenza di manager, un tutorial di 5 partite e la creazione del club,
  circa 30 minuti. Spuntate la casella "Salta il tutorial" in Launcher.exe prima di GIOCA: una carta nuova riceve un club già pronto
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
  P       pausa di una partita su questo PC (al massimo 20 secondi, poi riprende da sola; P o RIPRENDI la
          termina prima; la pausa successiva è possibile dopo 10 secondi). Non online, non con un'altra postazione
          collegata, non mentre il proiettore è acceso. Il tasto si cambia in KEYS.

ONLINE (VS tra due case)
  1. In Launcher.exe scegliete Online. Aprite "Indirizzo diretto" e scrivete l'indirizzo del server (HOST o
     HOST:PORTA); la lobby interroga ogni indirizzo che avete scritto. Il server del progetto accetta solo gli indirizzi
     della sua lista consentita: chiedete a chi lo gestisce di aggiungere il vostro. Le SALE GIOCHI restano
     disattivate per il 2017-18; l'elenco è SFIDE VS.
  2. CREA SFIDA apre una stanza e mostra il suo codice (ad es. K7QM): comunicatelo all'amico, che sceglie quella
     stanza e preme ENTRA. PARTITA RAPIDA entra nella stanza aperta più vecchia ancora in attesa, oppure ne apre una.
     ESCI (Esc) lascia il posto.
  3. Ogni giocatore preme PRONTO. Quando entrambi sono pronti, un conto alla rovescia di 5 secondi avvia il gioco. Se
     siete pronti e nessuno arriva per circa 90 secondi, il gioco parte con una squadra CPU.
  4. Tasti della lobby: Su/Giù riga, Sinistra/Destra o Tab elenco, Invio entra o PRONTO, H CREA SFIDA, F5 aggiorna,
     Esc esce. Gamepad: croce direzionale o stick sinistro per muoversi, A = Invio, B = Esc, X = aggiorna,
     Y = CREA SFIDA, LB/RB = elenco.
  "Server club" (sotto Indirizzo diretto) indica dove è salvato il vostro club; vuoto significa il server scelto.
  "Collegamento VS diretto" (relay.wccf.it) collega il vostro PC a quello dell'altro giocatore. La prima volta Windows
  Firewall può chiedere di Launcher.exe: consentitelo per le connessioni dirette; se lo negate, il VS funziona comunque
  tramite il relay, con un po' più di ritardo. Togliete la spunta per disattivare il collegamento. Scegliete Questo PC
  per tornare al vostro club su questo PC.

COSA NON C'È IN QUESTA ALPHA
  - La modalità carriera.
  - Il gioco in LAN e le sale con più posti: un posto per PC. Online c'è solo il VS tra due case.
  - La scala di rendering.

PROBLEMI NOTI
  - Il programma di controllo può bloccarsi dopo circa 90 minuti di inattività. Chiudete il gioco e riavviatelo; il
    club resta salvato.
  - La registrazione della squadra può attendere circa 95 secondi prima di proseguire.
  - Il VS online tra due case non è ancora stato giocato per intero su due PC: consideratelo una prova.
  - I programmi non sono firmati. Windows SmartScreen può mostrare "Il PC è stato protetto da Windows": scegliete
    "Ulteriori informazioni", poi "Esegui comunque". Se Microsoft Defender segnala un file, non disattivate Defender:
    segnalate il nome del file e il messaggio di Defender (vedere LOG E SEGNALAZIONI).
  - Gli antivirus possono non gradire overlay\inject.exe (carica il pannello nel gioco) e bin\winmm.dll (sta tra il
    gioco e Windows). Il loro codice sorgente è in source\.

ANTIVIRUS E SMARTSCREEN
  I programmi non sono firmati. Per funzionare, Launcher.exe avvia i programmi del gioco con l'interfaccia di debug di
  Windows e vi carica due DLL di Forza-WCCF: forza_panel.dll disegna il pannello sul fotogramma del gioco; netguard.dll
  e wccfnet.dll delle copie protette sostituiscono le chiamate del gioco agli adattatori di rete, così il gioco non può
  far perdere a questo PC la concessione di rete (lease). La configurazione modifica copie dei programmi del gioco,
  mai gli originali, che salva in original\. Gli antivirus possono trattarlo come uno strumento per barare. I file di
  ogni versione sono inviati a Microsoft come falso positivo prima della pubblicazione.
  Se Defender segnala ancora un file: non disattivatelo. Annotate il nome del rilevamento, ripristinate il file dalla
  cronologia di protezione solo se proviene dal download ufficiale, e segnalatelo: <link per le segnalazioni: da
  compilare a cura del proprietario>
  SmartScreen: scegliete "Ulteriori informazioni", poi "Esegui comunque".

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
  english\, italian\   i testi inglese e italiano del gioco - solo testo
  source\    il sorgente C dei programmi in bin\ e overlay\
  data\      creata sul vostro PC: impostazioni, salvataggi (save\gamesvr), log
