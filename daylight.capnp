@0xa960fc22a4939250;

enum Language {
  unspecified @0;
  # Core tree-sitter libraries (projects hosted under tree-sitter org)
  agda @1;
  bash @2;
  c @3;
  # cpp
  # csharp
  # css
  # embedded-template
  # go
  # html
  # java
  # java
  # javascript
  # jsdoc
  # json
  # julia
  # php
  # python
  # regex
  # rust
  # typescript

  # Community contributions go here
}

enum ErrorCode {
  unspecified @0;
  timedOut @1;
  cancelled @2;
  unknownLanguage @3;
}

interface HtmlHighlighter {
  struct File {
    ident @0 :UInt16;
    filename @1 :Text;
    contents @2 :Data;
    options  @3 :List(Text);
    language @4 :Language;
  }

  struct Request {
    files @0 :List(File);
    timeoutMs @1 :UInt64;
  }

  struct Document {
    ident @0 :UInt16;
    filename @1 :Text;
    language @2 :Language;
    lines @3 :List(Text);
  }

  struct Failure {
    ident @0 :UInt16;
    reason @1 :ErrorCode;
  }

  struct Response {
    documents @0 :List(Document);
    failures @1 :List(Failure);
  }

  html @0 (request: Request) -> (response: Response);
}
