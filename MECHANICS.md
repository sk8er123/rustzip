# RustZip — behaviour sheet

RustZip is an archive manager, not a game, so this sheet lists the numbers and
rules that shape how it behaves. Clean room: nothing was taken from the
original program; every value comes from the person's request or is a
design choice (marked "estimated").

## Formats
| Value | Setting | Source |
|---|---|---|
| Open & extract | ZIP, 7Z, TAR, TAR.GZ, TAR.BZ2, TAR.XZ, GZ (also BZ2, XZ) | request |
| Create | ZIP only | request |
| Edit (add/delete) | ZIP only | request |
| Format detection | by file content (magic bytes), extension as fallback | estimated |

## Compression (new ZIP / add to ZIP)
| Value | Setting | Source |
|---|---|---|
| Compression levels | 0–9 (0 = store, 1–9 = Deflate) | request ("compression level option") |
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
| Language | Korean UI text | request |
