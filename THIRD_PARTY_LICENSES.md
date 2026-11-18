# 제3자 라이선스 고지

RustZip은 [MIT 라이선스](LICENSE)로 배포하지만, 아래 코드와 파일은 원래 저작자의 라이선스를 따릅니다.
각 항목의 라이선스 전문은 원본에서 그대로 옮겼습니다.

## 소스 코드에 포함된 것

### EggDotNet — MIT

- 출처: <https://github.com/akolman/EggDotNet>
- 사용한 곳
  - `src/engine/codec/azo.rs` — EggDotNet의 AZO 해독기를 Rust로 옮겼습니다.
  - `tests/fixtures/egg/` — EggDotNet의 `src/EggDotNet.Tests/test_files/`에 있는 EGG·ALZ 테스트 샘플입니다.
- EGG 형식과 LEA 구현은 EggDotNet을 참고했습니다.

```
MIT License

Copyright (c) 2026 Andrew Kolman (akolman)

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## 테스트 샘플

아래 파일은 테스트에만 쓰이며 RustZip 실행 파일에는 들어가지 않습니다.

### lhasa — ISC

- 출처: <https://github.com/fragglet/lhasa> (`test/archives/`)
- 사용한 곳: `tests/fixtures/lzh/`
- 샘플 자체는 Explzh, LArc 등 다른 LHA 프로그램이 만든 압축 파일입니다.

```
ISC License

Copyright (c) 2011-2025, Simon Howard

Permission to use, copy, modify, and/or distribute this software
for any purpose with or without fee is hereby granted, provided
that the above copyright notice and this permission notice appear
in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL
WARRANTIES WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE
AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR
CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM
LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT,
NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN
CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

### unrar.rs — MIT 또는 Apache-2.0

- 출처: <https://github.com/muja/unrar.rs> (`data/`)
- 사용한 곳: `tests/fixtures/rar/`
- MIT와 Apache-2.0 중 MIT 조건으로 사용합니다.

```
Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
```

### unarj-rs — Apache-2.0

- 출처: <https://github.com/mkrueger/unarj-rs> (`tests/data/`)
- 사용한 곳: `tests/fixtures/arj/`
- 라이선스 전문은 같은 폴더의 [`tests/fixtures/arj/LICENSE`](tests/fixtures/arj/LICENSE)에 있습니다. 이 파일은 ARJ 샘플을 푼 결과와 비교하는 데도 쓰입니다.

## Rust 의존 라이브러리

`Cargo.toml`의 의존 크레이트는 빌드할 때 내려받으며 각자의 라이선스를 따릅니다.
모두 MIT, Apache-2.0, BSD, Zlib 등 상업적 사용이 가능한 라이선스입니다.
빌드한 실행 파일을 배포할 때는 이 라이선스들의 고지도 함께 넣어야 합니다. [`cargo-about`](https://github.com/EmbarkStudios/cargo-about)으로 목록을 만들 수 있습니다.

### UnRAR — UnRAR 라이선스

RAR 해제에는 `unrar` 크레이트가 함께 빌드하는 RARLAB UnRAR 소스를 씁니다.
UnRAR 소스는 무료로 쓸 수 있지만 RAR 압축기를 만들거나 RAR 압축 알고리즘을 다시 만드는 데 쓸 수 없습니다. RustZip은 RAR 파일을 풀기만 합니다.

```
 ******    *****   ******   UnRAR - free utility for RAR archives
 **   **  **   **  **   **  ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
 ******   *******  ******    License for use and distribution of
 **   **  **   **  **   **   ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
 **   **  **   **  **   **         FREE portable version
                                   ~~~~~~~~~~~~~~~~~~~~~

      The source code of UnRAR utility is freeware. This means:

   1. All copyrights to RAR and the utility UnRAR are exclusively
      owned by the author - Alexander Roshal.

   2. UnRAR source code may be used in any software to handle
      RAR archives without limitations free of charge, but cannot be
      used to develop RAR (WinRAR) compatible archiver and to
      re-create RAR compression algorithm, which is proprietary.
      Distribution of modified UnRAR source code in separate form
      or as a part of other software is permitted, provided that
      full text of this paragraph, starting from "UnRAR source code"
      words, is included in license, or in documentation if license
      is not available, and in source code comments of resulting package.

   3. The UnRAR utility may be freely distributed. It is allowed
      to distribute UnRAR inside of other software packages.

   4. THE RAR ARCHIVER AND THE UnRAR UTILITY ARE DISTRIBUTED "AS IS".
      NO WARRANTY OF ANY KIND IS EXPRESSED OR IMPLIED.  YOU USE AT 
      YOUR OWN RISK. THE AUTHOR WILL NOT BE LIABLE FOR DATA LOSS, 
      DAMAGES, LOSS OF PROFITS OR ANY OTHER KIND OF LOSS WHILE USING
      OR MISUSING THIS SOFTWARE.

   5. Installing and using the UnRAR utility signifies acceptance of
      these terms and conditions of the license.

   6. If you don't agree with terms of the license you must remove
      UnRAR files from your storage devices and cease to use the
      utility.

      Thank you for your interest in RAR and UnRAR.


                                            Alexander L. Roshal
```
