# 한국어 인터페이스

**편집 → 언어 → 한국어** 또는 **설정 → 일반 → 언어**에서 선택합니다.
설정은 즉시 적용되고 다음 실행에도 유지됩니다(`ui.json`의 `language: "ko"`).
`ko`, `ko-KR`, `ko_KR.UTF-8` 같은 시스템 언어 태그도 인식합니다.

## 범위

- `crates/ui-egui/locales/ko.json`: 일반 메시지 2,340개, 원문과 다른 번역 2,330개.
- `crates/ui-egui/locales/ko-formats.json`: 형식 메시지 299개, 원문과 다른 번역 294개.
- `docs/whats-new.ko.md`: 현재 영어 릴리스 노트에 대응하는 한국어 문서.

2026-10-23 기준 전체 카탈로그 합집합 3,790개 중 한국어 카탈로그에서 원문과 다른
메시지는 2,624개(69%)입니다. 릴리스 노트는 별도 문서이므로 이 비율에 포함되지 않습니다.
새로 추가되었거나 번역하지 않은 메시지와 하위 계층의 기술 오류는 영어로 표시됩니다.
사용자가 입력한 파일명·폴더명·앨범명·메타데이터와 명령 ID는 번역하지 않습니다.

## 폰트와 빌드

영문 UI 글꼴은 기존 Inter를 유지하고, 한글에는 **Nanum Gothic Regular/Bold**를 사용합니다.
폰트 파일은 LightCraft에 복사하지 않으며, OFL 라이선스와 함께 별도의 craft-fonts 입력으로
포함합니다. 개인용 통합에서 사용하는 revision은
`7ef0ffc2291b3e7dc8dcfaa78ed8e51b0453f5b0`
([storytold/craft-fonts#35](https://github.com/storytold/craft-fonts/pull/35))입니다.
닫혔지만 upstream에 병합되지 않은 폰트 PR의 고정 revision입니다.
처음 검토한 다른 PR의 family 이름 불일치를 피하고, 파일 내부 이름과 일치하는
`NanumGothic` manifest를 사용합니다. 두 후보의 Regular/Bold 폰트 바이트는 같습니다.

### Windows에서 더블클릭으로 빌드

저장소 루트의 [`build-windows.cmd`](../build-windows.cmd)를 더블클릭합니다.
실행 위치와 관계없이 저장소로 이동하고, 나눔고딕이 포함된 craft-fonts를 준비한 뒤
앱과 CLI를 HEIF 지원이 포함된 **Windows x64 릴리스**로 빌드합니다.
첫 실행에는 인터넷 연결과 충분한 디스크 공간이 필요하며 오래 걸릴 수 있습니다.
완료 또는 오류 메시지는 창에 남습니다. 자동 실행에서는 `build-windows.cmd --no-pause`를
사용하면 종료 코드로 성공·실패를 확인할 수 있습니다.

필수 도구는 **Git for Windows**, **Rust MSVC 툴체인**,
**Visual Studio의 C++ 데스크톱 개발 도구와 Windows SDK**입니다.
스크립트는 도구를 자동 설치하거나 관리자 권한을 요청하지 않으며, 없으면 설치 안내를
표시합니다. MSVC 환경은 자동으로 설정하므로 개발자 터미널을 따로 열 필요가 없습니다.

완료 후 `target\x86_64-pc-windows-msvc\release\lightcraft.exe`를 더블클릭해 실행합니다.
`CARGO_TARGET_DIR`을 지정한 경우 해당 폴더 아래에 생성됩니다. 한국어는 앱의 언어 메뉴에서
선택합니다. 이 스크립트는 설치 파일 생성이나 커밋·푸시는 하지 않습니다.
기존 craft-fonts는 변경하지 않으므로 오래된 폰트 입력으로 빌드가 실패하면 아래의 고정
revision을 별도로 준비하고 `CRAFT_FONTS_DIR`로 지정하세요.

### 수동 개발 빌드

Windows PowerShell에서 저장소 루트 기준:

```powershell
git clone https://github.com/storytold/craft-fonts ..\craft-fonts
git -C ..\craft-fonts fetch origin 7ef0ffc2291b3e7dc8dcfaa78ed8e51b0453f5b0
git -C ..\craft-fonts checkout --detach 7ef0ffc2291b3e7dc8dcfaa78ed8e51b0453f5b0
$env:CRAFT_FONTS_DIR = (Resolve-Path ..\craft-fonts).Path
$env:CRAFT_FONTS_REQUIRED = '1'
cargo run -p lightcraft -- --control 7980
```

기존 craft-fonts 작업 디렉터리에 변경 사항이 있다면 위 checkout 전에 보존해야 합니다.
필수 폰트 빌드는 나눔고딕이 없으면 오류로 중단합니다. 폰트 입력을 생략한 개발 빌드는
계속 가능하지만 한국어 글리프를 보장하지 않습니다. Windows에 폰트를 설치하는 것만으로
앱의 임베디드 폰트 목록이 갱신되지는 않습니다.

데스크톱은 Regular/Bold를 포함하고 웹은 Regular를 포함합니다. 한자는 기존 CJK fallback을
사용하므로 한국식 한자 자형을 보장하지 않습니다. 현대 한글 완성형 11,172자는 검사하지만,
분해 자모·옛한글의 조합과 IME 입력을 이 변경으로 해결했다고 주장하지 않습니다.

## 검증과 유지보수

```powershell
cargo test -p lightcraft-engine nanum_gothic_covers_modern_hangul
cargo test -p lightcraft-ui-egui i18n::tests -- --nocapture
cargo xtask ci
```

언어 명령은 `app.language.korean`입니다. 메뉴·컨트롤 채널·설정 저장, 날짜·수량 표시,
한글·한자 혼용의 일반/굵은/고정폭 글리프를 검사합니다.
영어 릴리스 노트가 변경되면 한국어 문서의 날짜·항목도 함께 갱신해야 합니다.

번역은 영어 원문을 바탕으로 한
[storytold/lightcraft#2435](https://github.com/storytold/lightcraft/pull/2435)의 번역을 통합하고
현재 브랜치의 추가 메시지와 릴리스 노트를 보완한 것입니다. Adobe 언어 팩이나 폰트는
사용하지 않습니다.
