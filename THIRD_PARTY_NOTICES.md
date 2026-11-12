# Third-Party Notices

sTerm source code is licensed under Apache License 2.0. Third-party components remain subject to their own licenses.

## Runtime dependencies

| Component | Version | License selected or declared |
| --- | ---: | --- |
| Tauri (tauri, @tauri-apps/api) | 2.12.x | Apache-2.0 or MIT |
| Tauri clipboard manager | 2.5.0 | Apache-2.0 or MIT |
| portable-pty | 0.8.1 | MIT |
| serde / serde_json / uuid / base64 | lockfile-pinned | Apache-2.0 or MIT |
| xterm.js and its fit, search, web-links addons | 5.5.0 / lockfile-pinned | MIT |

The complete resolved dependency graph is recorded in src-tauri/Cargo.lock and package-lock.json. Those lockfiles are the version authority for a release.

## MIT-licensed JavaScript terminal components

The distributed web assets include xterm.js and its addons. They are licensed under the MIT License and include these copyright notices:

    Copyright (c) 2017-2019, The xterm.js authors (https://github.com/xtermjs/xterm.js)
    Copyright (c) 2014-2016, SourceLair Private Company (https://www.sourcelair.com)
    Copyright (c) 2012-2013, Christopher Jeffrey (https://github.com/chjj/)

## Windows console runtime

The release package redistributes these x64 runtime files:

- conpty.dll
- OpenConsole.exe

They were obtained from the node-pty 1.1.0-beta43 runtime distribution bundled with Microsoft VS Code. The supplied license is MIT; its complete text is included in every release as conpty-LICENSE.txt. Version and checksum details are in [src-tauri/resources/conpty/README.md](src-tauri/resources/conpty/README.md).

## License texts

The following is the MIT License text applicable where a component selects or declares MIT:

    MIT License

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

Apache-2.0 terms are included in the root [LICENSE](LICENSE). This notice is an attribution summary, not legal advice; review new dependencies and their license obligations before release.
