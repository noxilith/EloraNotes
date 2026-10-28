# App Store listing

Everything App Store Connect asks for, ready to paste. Fields marked **you** need a decision or
a login only you have.

## App information

| Field | Value |
| --- | --- |
| Name | EloraNotes (**you**: the name must be unique store-wide; if it is taken, add a suffix here, the home-screen name stays EloraNotes) |
| Subtitle (30) | Linked Markdown notes |
| Primary category | Productivity |
| Secondary category | Utilities |
| Bundle ID | io.github.noxilith.EloraNotes |
| Price | Free |
| Copyright | 2026 noxilith |
| Privacy policy URL | https://noxilith.github.io/docs/privacy/ |
| Support URL | https://github.com/noxilith/EloraNotes/issues |
| Marketing URL | https://noxilith.github.io/docs/ |

## Version 1.0

**Promotional text (170)**

A calm, fast notes app. Plain Markdown files, [[wiki links]], backlinks, a graph of how your notes connect, and optional live sharing with the people you choose.

**Description (4000)**

EloraNotes is a calm place to write. Every note is a plain Markdown file that you own, with no account and nothing to sign up for.

WRITE
• Markdown that is styled as you type: headings, bold, italic, strikethrough, code, highlights, task lists, quotes
• Folders, tags, pinned notes and fast search
• Six themes: Obsidian, Paper, Nordic, Matcha, Sepia and Midnight

CONNECT
• [[Wiki links]] between notes, with a "Linked from" list under each note
• A graph of your notes: linked notes gather into coloured clusters, loose notes sit on an arc below. Pinch to zoom, drag to move, tap a note to light up its links

SHARE (OPTIONAL)
• Connect to a shared library on a server you run (the free EloraNotes server) and edit notes together in real time
• See everyone else's cursor, in their colour, with their name, as they type
• No cloud service in between: your notes go only to your own server

PRIVATE BY DESIGN
• No account, no analytics, no ads, no tracking
• Notes are Markdown files in the EloraNotes folder of the Files app, so they are easy to back up or move
• Open source: github.com/noxilith/EloraNotes

EloraNotes also runs on Windows, macOS and Linux.

**Keywords (100)**

notes,markdown,wiki,links,graph,backlinks,obsidian,journal,writing,folders,collaborate,zettelkasten

**What's New (1.0)**

First release.

## Age rating

Answer **None** to every question (no violence, no mature content, no unrestricted web access, no
user-generated content shared with strangers). The result is 4+.

## App Privacy ("nutrition label")

**Data Not Collected.** The app has no analytics or third-party SDKs and sends nothing to the
developer. The optional shared library sends notes to the user's own server, which is not the
developer. (See the privacy policy.)

## Export compliance

The app only uses the encryption built into the operating system and standard TLS to reach the
user's own server. `ITSAppUsesNonExemptEncryption` is already set to `false`, so the build is not
held for the export-compliance question.

## App Review information

**Sign-in required:** No.

**Notes for the reviewer (paste this):**

> EloraNotes needs no account. Everything works offline with notes on the device: create a note
> with the compose button, link notes with [[double brackets]], open the graph with the button
> next to compose, and change the theme under Settings (the gear).
>
> The optional "Shared library" in Settings connects to a server run by the user (the open-source
> elora-server, https://github.com/noxilith/EloraNotes). To try it, use: **you:** server address,
> port, password, a name and an identifier for a demo server that is online during review.
> The app does not send anything anywhere until a server is entered.

(**you:** App Review may ask to test the shared library. Put a small demo server online for the
review and fill in its details above.)

**Contact:** your name, phone and email (App Store Connect asks for them; they are not shown).

## Screenshots

Required: **6.9-inch iPhone**, 1320 x 2868. The iOS workflow already takes them with the right
simulator: download the `iphone-screenshots` artifact from a run of the **iOS** workflow. Good
ones to upload, in this order: `list-obsidian`, `graph-obsidian`, `note-obsidian`,
`list-paper`, `graph-paper`, `search-obsidian`. (The app is iPhone-only, so no iPad screenshots
are needed.)

## Before pressing "Submit for review"

1. TestFlight build works on a real iPhone, including the shared library.
2. Fill in the sections above in App Store Connect and choose the build.
3. Under **Trader status** (EU) say whether you are a trader. A free hobby app with no income is
   normally "not a trader"; read Apple's wording and decide.
