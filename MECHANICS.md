# RustZip — behaviour sheet

RustZip is an archive manager, not a game, so this sheet lists the numbers and
rules that shape how it behaves. Clean room: nothing was taken from the
original program; every value comes from the person's request or is a
design choice (marked "estimated").

## Formats
| Value | Setting | Source |
|---|---|---|
| Open & extract | ZIP, 7Z, RAR, ALZ, EGG, LZH, ARJ, CAB, ISO, TAR, CPIO, AR; single files and TAR/CPIO/AR inside GZ, BZ2, XZ, LZMA, LZ, ZST, LZ4, BR, SZ, Z | request ("모든 압축 알고리즘") |
| Create | ZIP, 7Z, TAR, TAR.(GZ/BZ2/XZ/ZST/LZ4/LZMA/LZ/BR/SZ), single-file GZ/BZ2/XZ/ZST/LZ4/LZMA/LZ/BR/SZ | request |
| Not creatable | RAR (closed format), ALZ/EGG, LZH, ARJ, CAB, ISO, .Z | estimated |
| Edit (add/delete) | ZIP only | request |
| Format detection | by file content (magic bytes); Brotli and LZMA-alone also need the extension; self-extracting .exe checked for ZIP/RAR | estimated |
| Model | container (ZIP, 7Z, TAR...) × outer filter (GZ, ZST...) | estimated |

## Compression (new archive / add to ZIP)
| Value | Setting | Source |
|---|---|---|
| Format menu | ZIP, 7Z, TAR, TAR.*, single-file filters; changing it changes the file name ending | estimated |
| ZIP methods | Deflate (default), BZip2, XZ, Zstandard, PPMd, 저장 | request |
| 7Z methods | LZMA2 (default), LZMA, PPMd, BZip2, Deflate, Zstandard, Brotli, LZ4, 저장 | request |
| 7Z + password | AES-256, file list encrypted too; Brotli not offered with a password (sevenz-rust2 reads past the stream into the AES padding) | estimated |
| Single-file formats | exactly one file; name is the file's own name plus the ending (`보고서.txt.zst`) | estimated |
| Compression levels | 0–9 (0 = store/fastest); mapped to each algorithm's own range (zstd 1–19, brotli 0–11) | request ("compression level option") |
| Default level | 6 ("보통") | estimated |
| Password encryption | AES-256 (WinZip AE-2) | request |
| Password confirmation | typed twice when creating | estimated |
| New file names | UTF-8 with the UTF-8 flag | estimated |
| Same name when adding | replaces the old entry | estimated |

## Korean file names
| Value | Setting | Source |
|---|---|---|
| Default name encoding | 자동 감지: UTF-8 flag → valid UTF-8 → CP949 → CP437 | request (CP949/EUC-KR) |
| Manual choices | 자동, UTF-8, CP949/EUC-KR, CP437 | estimated |
| Editing keeps old names | raw copy, so CP949 names stay CP949 | estimated |

## Extracting
| Value | Setting | Source |
|---|---|---|
| Default destination | folder of the archive, plus a new folder named after it | estimated |
| Same-name files | 이름 바꿔 풀기 (default), 덮어쓰기, 건너뛰기 | estimated |
| Selected extract | paths are relative to the folder being viewed | estimated |
| Unsafe paths (`..`, drive letters) | skipped and counted | estimated (safety) |
| Double-click a file | extracts to a temp folder and opens it | estimated |

## Window & controls
| Value | Setting | Source |
|---|---|---|
| Window size | 1100 × 700 (min 720 × 420) | estimated |
| File list columns | 이름, 크기, 압축 크기, 압축률, 수정한 날짜 | request (+압축률 estimated) |
| 압축률 | packed size ÷ original size × 100 % | estimated |
| Sorting | click a column header; folders always first | estimated |
| Toolbar | 열기, 새로 압축, 압축 풀기, 추가, 삭제, 파일 열기, 상위 폴더, 정보 | estimated |
| Shortcuts | Ctrl+O open, Ctrl+N new, Ctrl+E extract, Del delete, Backspace up, Enter open, Ctrl+A select all | estimated |
| Selection | click, Ctrl+click toggle, Shift+click range | estimated |
| Drag and drop | one archive → open it; other files → new ZIP, or add to the open ZIP | request |
| Command line | first argument is opened at start | request |
| Progress window | appears when a job runs longer than 250 ms, with 취소 | request (delay estimated) |
| Language | 한국어, English — 보기 › 언어 / Language 메뉴에서 바꾸면 바로 적용되고 저장됨 | request |
| Starting language | `--lang ko\|en` → saved choice → OS language (Korean if the OS is Korean, otherwise English) | estimated |
| Saved in | Windows `%APPDATA%\RustZip\settings.txt`, macOS `~/Library/Application Support/RustZip/settings.txt`, Linux `~/.config/rustzip/settings.txt` | estimated |
| Library error messages | recognised by type (file system, ZIP, 7z) and shown in the current language; unknown ones keep their original text after a translated lead-in | request |
