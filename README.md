# Minecraft Native

Minecraft Java Edition 26.3 싱글플레이를 Rust로 재구현하기 위한 프로젝트입니다. **현재는 플레이 데모와 호환성 기반만 구현되어 있으며, 완성된 마인크래프트가 아닙니다.**

## 실행

Windows x64와 로컬에 설치된 Minecraft Java Edition 26.3이 필요합니다. 기본 설치 경로는 `%APPDATA%\.minecraft`입니다. 26.3 또는 런처 자산이 없으면 메인 화면에서 설치 폴더를 직접 선택할 수 있습니다. 선택한 경로와 리소스팩 목록은 `%APPDATA%\MinecraftNative\options.json`에 저장됩니다. 정품 게임 파일은 읽기만 하며 프로젝트에 복사하거나 배포하지 않습니다.

완성된 게임이 아닌 현재 실험 버전의 EXE는 다음처럼 빌드한 뒤 `target\release\native-client.exe`를 실행합니다. 빌드에는 Rust stable과 MSVC 빌드 도구가 필요하지만, 실행에 별도의 프로젝트 파일이나 명령줄 옵션은 필요하지 않습니다. Minecraft 설치와 GPU 드라이버는 별도로 필요합니다.

```powershell
cargo build --release -p native-client
```

Re-Craft의 자체 버전은 Minecraft 기준 버전 26.3과 별도로 관리하며 메인 화면 왼쪽 아래에 함께 표시합니다. 로컬 빌드의 기본 버전은 워크스페이스 `Cargo.toml`의 `0.1.0`이고, GitHub Actions 릴리스 빌드는 해당 릴리스 버전을 실행 파일에 반영합니다.

커밋 제목은 `type(scope): subject` 형식을 사용합니다. 예를 들면 `feat(world): add chunk generation`, `fix(camera): correct vertical view`입니다. 범위는 영문 소문자·숫자와 `-`, `_`, `.`, `/`를 사용합니다. 호환성을 깨는 변경은 `feat(world)!: ...` 또는 본문의 `BREAKING CHANGE: ...`로 표시합니다. 허용되는 유형은 `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`, `style`, `revert`입니다. Actions는 새로 push된 일반 커밋을 검사하고 병합 커밋은 제외합니다. 기존 이력은 재작성하지 않습니다.

모든 브랜치의 push에서 Windows x64 테스트와 릴리스 빌드를 수행하고 EXE를 Actions 아티팩트로 보관합니다. 기본 브랜치의 빌드가 성공하면 같은 EXE를 GitHub Release에 게시합니다. 릴리스 태그가 없다면 `0.1.0`을 기준으로 시작하며, 마지막 게시 릴리스 이후의 커밋 중 `feat`가 있으면 부 버전, 호환성을 깨는 변경이 있으면 주 버전을 올립니다. `fix`, `perf`와 나머지 유형만 있으면 패치 버전을 올립니다. 여러 커밋이 한 번에 포함되면 가장 큰 변경 한 가지만 적용합니다. 새 브랜치 생성 또는 강제 push의 이전 커밋을 읽을 수 없는 경우 커밋 형식 검사는 최신 커밋만 수행합니다. 이미 릴리스된 커밋을 다시 실행하면 같은 버전을 사용합니다. 태그 push는 별도의 빌드를 시작하지 않습니다.

릴리스 게시에는 저장소의 GitHub Actions 실행 및 `contents: write` 권한, 릴리스 태그 생성 권한이 필요합니다. Actions에서 붙이는 EXE는 Windows C 런타임을 정적으로 연결하며 Minecraft 파일은 포함하지 않으므로 사용자의 26.3 설치가 필요합니다. Actions 큐에는 브랜치별로 최대 100개 실행이 대기할 수 있습니다.

여러 push의 실행 순서는 GitHub에서 보장하지 않습니다. 뒤의 커밋이 이미 게시된 뒤 이전 커밋의 실행이 시작되면 이전 실행은 빌드 아티팩트만 만들고 정식 릴리스는 생략해 오래된 코드가 최신 릴리스가 되지 않도록 합니다.

메인 화면의 `Singleplayer`에서 데모를 실행하거나 26.3 월드를 선택할 수 있습니다. 기존 월드 실험 플레이는 **저장하지 않으며**, 변경 사항은 종료 시 사라집니다. `Options`에서 설치 폴더와 리소스팩을 바꿀 수 있습니다. 메뉴는 설치된 26.3의 로고, 버튼, 글꼴과 파노라마를 읽어 표시합니다. 리소스팩으로 파노라마를 바꿀 때 여섯 면은 크기가 같은 정사각형이어야 합니다. 아직 구현되지 않은 월드 생성과 대부분의 설정은 비활성 상태입니다. 읽기 전용 뷰어와 청크 좌표 지정은 개발 빌드에서만 제공합니다.

기존 월드의 지형 조명은 저장된 월드 시계와 지원되는 차원 시간선에 따라 달라집니다. 오버월드형 차원의 하늘 배경색도 주변 바이옴과 시계에 따라 바뀌며, 하늘이 없는 차원은 주변 바이옴의 안개색을 배경에 사용합니다. 해당 차원의 지형 안개 거리에는 차원·바이옴 설정과 지원되는 시간선을 적용합니다. 배경은 아직 단색이고 안개·태양·달·날씨와 엔드 하늘은 불완전합니다. 바이옴별 지형 조명이나 지원하지 않는 시간선 설정이 있으면 정적 조명을 사용합니다.

기존 월드에 저장된 지원 범위의 경험치 구슬은 표시하고 획득할 수 있으며, 저장된 구슬의 묶음 수와 수명도 반영합니다. 물속 부력, 유체 흐름과 용암 반동·피해 및 화상을 처리하지만 실제 월드에서의 전체 유체 동작과 난수는 아직 검증되지 않았습니다. 가까운 구슬의 주기적인 병합을 지원하나 임시 엔티티 ID를 사용하므로 바닐라와 병합 결과가 다를 수 있습니다. 플레이 중 변경 사항은 저장하지 않습니다.

생존·모험 모드에는 26.3 또는 선택한 리소스팩의 체력과 허기 아이콘이 표시됩니다. 기존 월드에서는 저장된 체력·흡수·허기와 최대 체력 속성, 하드코어 여부를 읽으며 허기가 6 이하이면 달릴 수 없습니다. 일반 음식의 섭취와 기본적인 포만도·탈진·회복·굶주림을 처리합니다. 낙하 피해는 착지 거리, 물, `fall_damage` 게임 규칙, 안전 낙하 속성, 점프 강화·저항 효과와 장비의 지원되는 피해 방어 인챈트를 반영합니다. 낙하·익사·굶주림으로 사망하면 사망 화면을 표시하고, 지원되는 인벤토리와 사망 관련 게임 규칙을 메모리에 반영합니다. 기존 월드에서 플레이 중 새로 사망했을 때 같은 차원의 침대 또는 리스폰 정박기를 안전하게 확인할 수 있는 경우 부활할 수 있습니다. 먼 스폰의 청크는 백그라운드에서 읽으며 준비가 끝나면 부활할 수 있습니다. 다른 차원의 스폰, 월드 기본 스폰으로의 대체와 이미 사망한 채 저장된 플레이어의 부활은 아직 지원하지 않습니다. 기존 월드는 저장하지 않습니다. 다른 피해 유형과 피해 무적 시간, 블록의 추가 착지 동작, 일반적인 상태 효과 및 특수 음식도 아직 구현되지 않았습니다.

```powershell
cargo run -p native-client
```

메인 화면이나 게임 중 `Esc`로 연 일시정지 메뉴에서 `Options...` → `Video Settings...`를 선택해 `VSync`와 `Max Framerate`를 바꿀 수 있습니다. 프레임 제한은 10~250FPS 또는 무제한이며 데모와 기존 월드 실험 플레이에 즉시 적용됩니다. VSync를 끄면 화면 찢김이 발생할 수 있으며 GPU가 비동기 표시 방식을 지원하지 않으면 동기화가 유지될 수 있습니다. 값은 Minecraft Native 설정에 저장됩니다. 월드 밖 메뉴의 일반적인 갱신은 26.3처럼 60FPS를 기준으로 예약합니다. 자리 비움에 따른 추가 제한과 다른 비디오 설정은 아직 미구현입니다.

같은 화면의 `Render Distance`는 2~32청크를 선택하며 기본값은 12입니다. 기존 월드에서는 저장된 가까운 청크를 먼저 열고 나머지를 백그라운드에서 읽습니다. 플레이 중 설정을 바꾸면 청크 로딩 범위가 갱신되며, 아직 생성되지 않은 청크는 생성하지 않습니다. 월드 생성, 정확한 바닐라 청크 가시 범위와 시뮬레이션 거리 분리는 아직 구현되지 않았습니다.

같은 `Options...` 화면의 FOV 슬라이더는 30~110도를 선택하며 데모와 기존 월드 화면에 적용됩니다. 기본값은 70도이고, 일반적인 달리기 시야 변화도 반영합니다. 손 모델은 월드 시야와 별도의 기본 투영을 사용합니다. 물속·효과·특수 아이템 등 나머지 시야 변화는 아직 구현되지 않았습니다.

`Options...` → `Accessibility Settings`에서 `View Bobbing`을 켜거나 끌 수 있습니다. 걷기 흔들림은 월드와 손 모델에 적용되며 설정은 저장됩니다. 접근성 화면의 다른 항목은 아직 구현되지 않아 비활성화되어 있습니다.

게임 화면을 클릭하면 마우스가 잡힙니다. `WASD`로 이동하고, 마우스로 시점을 바꾸고, `Space`로 점프합니다. `Ctrl`을 누르거나 `W`를 빠르게 두 번 누르면 달리고, `Shift`를 누르면 웅크립니다. 좌클릭을 유지하면 블록을 파괴하고 우클릭으로 설치하며, `1`~`9`로 단축바를 선택합니다. `F`로 주 손과 보조 손의 아이템을 교체할 수 있습니다. 주 손이 비어 있을 때는 보조 손의 지원되는 블록을 설치합니다. `Q`는 선택한 아이템 한 개를, `Ctrl+Q`는 묶음 전체를 버립니다. `E`로 인벤토리를 열고 닫고, GUI에서 시작한 게임은 `Esc`로 인벤토리를 닫거나 일시정지할 수 있습니다. 데모에서는 설치한 블록을 소모하고, 기본 잔디·흙·돌·돌 벽돌과 판자 13종·작업대는 파괴 진행도와 균열을 표시합니다. 기본 잔디와 흙을 일반 아이템이나 빈손으로 파괴하면 흙이 떨어지고, 판자와 작업대는 자기 자신이 떨어져 가까이 가면 획득할 수 있습니다. 돌과 돌 벽돌은 적절한 도구 없이는 아이템이 떨어지지 않습니다. 임시 월드는 종료할 때 초기화됩니다.

인벤토리는 설치된 게임의 화면 텍스처와 저장된 플레이어의 슬롯·선택 위치·장비를 읽습니다. 플레이어 미리보기는 26.3 기본 스킨을 설치본 또는 선택한 리소스팩에서 읽고 마우스를 따라 회전합니다. 기존 월드에서는 저장된 싱글플레이 UUID로 기본 스킨을 선택합니다. 주 손과 보조 손에 든 단순 입체 모델과 생성된 평면 아이템 모델을 표시하고 보조 손 슬롯을 단축바에 표시합니다. 아이템 모델은 선택한 리소스팩을 적용해 백그라운드에서 읽습니다. 검·도구·음식의 스프라이트와 원목·판자·계단 같은 일반 입체 모델을 단축바·인벤토리·제작 결과에 표시합니다. 아이콘은 모델을 읽는 백그라운드 작업에서 만들어 캐시합니다. 방어구 아이콘은 장식 재료에 따른 모델 선택·팔레트 변환과 가죽 색상을 적용합니다. 인벤토리 플레이어 미리보기에는 기본 방어구의 착용 모델, 가죽 염색과 방어구 장식이 표시됩니다. 장비 텍스처와 정의는 선택한 리소스팩을, 장식 정의는 활성 데이터팩을 사용합니다. 특수 아이템 모델과 다른 모델 조건, 사용자 지정 스킨, 머리 장식·겉날개와 인챈트 빛남은 아직 구현되지 않았습니다. 일반 슬롯에 마우스를 올리고 `1`~`9` 또는 `F`를 누르면 해당 단축바·보조 손 슬롯과 교체합니다. 지원되는 제작 결과도 빈 대상 슬롯으로 옮길 수 있습니다. 나무, 기본 재료와 음식, 일곱 재질의 검·도구를 포함한 189가지 아이템은 일반 슬롯의 좌우 클릭과 Shift 클릭 이동을 지원합니다. 인벤토리의 2×2 칸과 작업대의 3×3 칸은 설치된 26.3과 활성 데이터팩의 일반 조합형·무형 제작법 및 아이템 태그를 읽습니다. 현재는 동작이 확인된 아이템만 재료와 결과로 처리하므로 예를 들어 원목으로 판자, 판자로 막대기와 작업대를 만들고 돌로 돌 버튼과 돌 벽돌을 만들 수 있습니다. 제작 결과의 일반 좌우 클릭과 Shift 클릭을 지원합니다. 특수 제작법, 구성 요소와 제작 부산물은 아직 지원하지 않습니다. 돌 벽돌·조약돌·판자·작업대와 원목 계열 50종은 설치할 수 있습니다. 원목을 놓은 면에 따라 축이 정해집니다. 작업대를 우클릭하면 설치된 게임의 화면 텍스처를 사용하는 제작 화면이 열리며, 블록이 사라지거나 플레이어가 멀어지면 재료를 반환하고 화면을 닫습니다. 제작법 책은 저장된 해금 목록과 설치본의 텍스처를 사용하며, 검색·분류·제작 가능 필터·페이지 이동과 클릭 또는 Shift 클릭으로 재료를 배치합니다. 재료가 부족한 레시피는 제작 칸에 유령 아이콘을 표시합니다. 일반적인 단일 재료 획득 발전 과제로 해금되는 제작법도 반영합니다. 묶인 레시피는 우클릭으로 펼쳐 각 재료 조합을 선택할 수 있으며, 패널 이미지는 리소스팩의 GUI 스프라이트 설정을 따릅니다. 복잡한 발전 과제와 검색창의 일부 편집 동작은 아직 지원하지 않습니다. 돌 버튼은 기존 월드에서 지원되는 완전 블록과 반 블록·계단의 지지 가능한 면에 설치하고 눌러 20틱 동안 켤 수 있으며 일반 도구로 채굴할 수 있습니다. 짧은 풀·양치식물·마른 덤불·불과 일반 물·용암의 칸은 설치 시 교체할 수 있습니다. 클릭한 면이 지지하지 못하면 시선 순서에 따라 다른 지원 면을 찾으며, 저장된 돌 버튼 예약 틱도 읽습니다. 지지 블록이 공기 또는 지원되는 반 블록·계단으로 바뀌어 지지력을 잃으면 버튼이 떨어지며 데이터팩 전리품을 적용합니다. 돌 버튼의 누름과 복귀 소리는 설치본 또는 선택한 리소스팩에서 읽으며 거리와 좌우 방향은 근사해 재생합니다. 다른 지지 형상과 지지 변경, 정확한 공간 음향과 레드스톤은 아직 지원하지 않습니다. 지원되는 아이템은 인벤토리 슬롯 위에서 `Q`·`Ctrl+Q`로 버릴 수 있으며, 커서에 든 아이템은 인벤토리 밖을 좌클릭해 전부, 우클릭해 하나를 버릴 수 있습니다. 제작 결과 슬롯 버리기와 아이템 전용 클릭 동작은 미구현입니다. 지원되는 아이템의 최대 묶음 크기와 사용자 데이터, 도구의 손상 값을 보존합니다. 손상된 지원 도구는 인벤토리와 단축바에 내구도 막대를 표시합니다. 아이콘 표시를 지원하지 않는 아이템은 자홍색 무늬로 표시하고, 상호작용을 해석하지 못한 아이템은 일반 클릭을 제한합니다. 안전하게 처리할 수 있는 양손·슬롯 교체에서는 저장된 구성 요소를 보존합니다. 인벤토리를 닫을 때 커서와 제작 칸의 아이템은 빈 공간으로 되돌립니다. 공간이 부족하면 지원되는 아이템의 남은 수량을 플레이어 앞에 떨어뜨립니다. 아직 해석하지 못한 아이템에 대해서는 안전하게 처리할 수 있을 때까지 닫기를 제한합니다. 기존 월드는 여전히 저장되지 않습니다.

일반 도끼로 원목을 우클릭하면 축을 유지하며 껍질을 벗깁니다. 일반 삽으로 흙 위에 공간이 있으면 흙길을 만들고, 괭이로 흙·잔디·흙길을 경작지로 바꿀 수 있습니다. 거친 흙은 괭이로 일반 흙이 되며, 뿌리내린 흙은 흙으로 바뀌면서 늘어진 뿌리가 떨어집니다. 도구 변환과 블록 태그, 단순 상호작용 전리품은 설치본과 활성 월드 데이터팩에서 읽습니다. 생존 모드에서는 사용한 손의 도구 내구도가 줄고 마지막 내구도라면 도구가 사라집니다. 주 손 도구가 동작하지 않았을 때 보조 손의 지원 도구를 시도하며, 웅크리지 않은 상태에서 보조 손에 방패처럼 아직 해석하지 못한 아이템이 있으면 주 손 도구의 사용을 제한합니다.

흙길과 경작지는 바닐라처럼 일반 블록보다 1/16블록 낮으며, 지원되는 고체 블록으로 위를 덮으면 다음 틱에 흙으로 돌아갑니다. 경작지의 물 공급·건조·작물 성장·밟기와 모든 이웃 블록 판정은 아직 구현되지 않았습니다. 복잡한 데이터팩 조건과 전리품, 소리·입자, 모든 아이템의 사용 우선순위도 남아 있습니다. 떨어진 뿌리의 모양은 근사치입니다.

지원되는 일반 아이템을 든 상태에서 저장 슬롯이나 제작 칸을 좌클릭 드래그하면 균등하게 나누고, 우클릭 드래그하면 한 개씩 배치합니다. 드래그 중 예상 수량과 커서에 남을 수량을 표시합니다. 플레이어 인벤토리의 방어구 슬롯에서는 기본 장비의 클릭·Shift 클릭·단축키 교체·버리기를 지원하며, 장비에 적용된 귀속 방지 효과는 활성 데이터팩에서 읽습니다. 인챈트와 일부 사용자 장비 구성 요소도 보존합니다. 엔티티 태그로 지정된 장착 제한과 일부 특수 아이템은 아직 지원하지 않습니다. 일반 드래그 960가지와 장비 조작 1,120가지를 Java 26.3 메뉴와 비교했습니다. 합성 월드의 실제 창에서 투구와 부츠 장착 및 잘못된 슬롯 거부를 확인했습니다. 장식 팔레트의 투명도·중복 색상 1,024개 표본도 Java와 비교했습니다. 전체 아틀라스 소스와 필터는 아직 지원하지 않습니다.

생존·모험 HUD에는 저장된 방어 속성과 현재 장비의 기본·사용자 속성 및 지원되는 데이터팩 인챈트 효과를 합산한 방어 수치가 표시됩니다. 아이콘과 리소스팩 재정의를 사용하며 장비 교체 후 바뀐 수치만 다시 그립니다. 수정자 ID가 중복되거나 장비 효과를 해석하지 못하면 방어 수치를 숨기고 로그에 기록합니다. 플레이어의 다른 화면에서 장비 외형과 실제 피해 감소는 아직 구현되지 않았습니다. 기존 월드의 변경 사항도 계속 저장되지 않습니다.

게임 화면에서는 주 손이 비어 있을 때 기본 스킨의 팔을, 들고 있을 때 지원되는 입체·스프라이트 아이템을 표시합니다. 보조 손도 지원되는 아이템을 표시하며 활성 리소스팩과 주 손 설정을 따릅니다. 기본 아이템의 공격·설치 손 흔들기와 양손의 아이템 교체 높이를 고정 틱으로 계산하고 프레임 사이를 보간합니다. 기본 흔들기 상태와 팔·아이템 변환, 대표적인 장착 전환을 Java 26.3과 비교했습니다. 아이템별 흔들기 구성 요소, 공격 속도와 상태 효과에 따른 변화, 모델별 교체 규칙, 특수 아이템, 스킨 레이어 설정은 아직 지원하지 않으며 조명은 근사치입니다. 스프라이트의 애니메이션은 첫 프레임만 표시합니다. 아이템 정의의 추가 변환과 일부 색상 지정 방식은 아직 지원하지 않으며, 입체 아이콘의 픽셀 및 투명도 처리는 바닐라 화면과의 대조가 남아 있습니다. 관전자 모드에서는 손을 숨깁니다.

DirectX 12 초기화를 먼저 시도하고 실패하면 Vulkan을 사용합니다. 개발 빌드에서는 선택된 백엔드와 FPS·TPS를 창 제목에 표시합니다. 데모의 지형과 물리는 바닐라와의 동등성이 아직 검증되지 않았습니다.

플레이 시점은 고정 틱 사이의 위치와 눈높이를 보간하고, 마우스 회전은 다음 틱 전에 화면에 반영합니다. 이동·블록 상호작용은 계속 고정 틱으로 처리합니다. 위치·눈높이와 기본 달리기 시야 계산, 합성 이동에서의 걷기 흔들림 행렬은 설치된 26.3 Java 카메라와 비교했습니다. 실제 플레이 전체의 걷기 흔들림, 다른 시야 효과와 연속 마우스 입력의 바닐라 대조는 남아 있습니다.

일반 마우스 시점은 Java 26.3의 기본 감도에 맞춰 계산합니다. 감도를 직접 변경하거나 마우스 축을 반전하는 설정과 부드러운 카메라·스코프 처리는 아직 구현되지 않았습니다.

메뉴 파노라마와 흐림은 GPU에서 그리며, 리소스와 월드 목록은 백그라운드에서 읽습니다. GPU 장치 초기화와 화면 버퍼 대기는 별도 렌더 스레드에서 처리하고, 바뀐 버튼과 최신 프레임만 전달해 메뉴 입력이 화면 대기에 묶이지 않도록 합니다. 메뉴 GPU를 초기화할 수 없거나 실행 중 사용할 수 없게 되면 소프트웨어 방식으로 전환합니다. 소프트웨어 파노라마와 흐림도 별도 스레드에서 화면 해상도로 계산하며 오래된 화면·크기의 결과는 버립니다. 데모나 기존 월드를 준비하는 동안에도 창에 접속 메시지를 표시하며, 준비가 실패하면 월드 목록으로 돌아가 오류를 표시합니다. Windows 실제 창에서 지연된 초기화, 오류 복구, 크기 변경 및 월드 전환을 검사했으며, 실제 화면 표시 간격과 바닐라 화면의 시각적 일치는 아직 별도 검증이 필요합니다.

## 현재 제공하는 도구

다음 명령행 검사와 그래픽 스모크 테스트는 개발 빌드 전용이며 릴리스 EXE에는 포함되지 않습니다. 일반 실행은 위 GUI를 사용합니다.

설치 버전, JAR 해시와 데모 텍스처를 검사합니다.

```powershell
cargo run -p native-client -- --check-install
```

폴더 또는 ZIP 리소스팩을 추가할 수 있으며, 여러 번 지정하면 마지막 팩의 우선순위가 가장 높습니다. **현재 데모에는 기본 지형·판자·작업대의 정사각형 텍스처 교체가 적용됩니다.** 데모의 애니메이션 텍스처는 첫 프레임만 표시하고 모델 재정의와 다른 리소스팩 기능은 아직 데모 렌더러에 연결되지 않았습니다.

```powershell
cargo run -p native-client -- --resource-pack "D:\Packs\my-pack.zip"
cargo run -p native-client -- --inspect-model minecraft:block/grass_block
cargo run -p native-client -- --inspect-blockstate minecraft:oak_stairs --property facing=east --property half=bottom --property shape=straight --property waterlogged=false
```

`--inspect-blockstate`는 해당 상태의 가중 모델과 다중 파트, 모델 요소의 회전, 직각 회전·UV 잠금과 텍스처 아틀라스 구성을 검사합니다.

26.3 월드의 메타데이터나 차원별 청크를 **읽기 전용**으로 검사합니다.

```powershell
cargo run -p native-client -- --inspect-world "D:\WorldCopies\my-world"
cargo run -p native-client -- --inspect-world "D:\WorldCopies\my-world" --chunk -1 0
cargo run -p native-client -- --inspect-world "D:\WorldCopies\my-world" --dimension minecraft:the_nether --chunk 0 0 --radius 1
```

현재 NBT, Anvil 위치 테이블, 외부 청크, GZIP·Zlib·무압축·Java LZ4, 26.3의 문자열·속성 객체가 혼합된 블록 상태 팔레트, 바이옴 팔레트와 조명 배열 판독을 지원합니다. 원본 NBT와 알 수 없는 블록 상태를 보존하며 누락된 청크와 공기를 구별합니다. 알 수 없는 압축 형식이나 손상된 데이터는 오류로 처리합니다. 청크 검사는 데이터 버전 5023만 허용합니다.

저장 엔진에는 실험적인 무변경 청크 왕복 기능이 있습니다. Java와 같은 월드 잠금을 잡고 원본 영역 파일 및 외부 청크를 `.minecraft-native-backups`에 백업한 다음 별도 임시 파일을 교체합니다. 지원하지 않는 형식이나 손상, NBT를 동일한 바이트로 복원할 수 없는 경우에는 저장을 중단합니다. 블록 팔레트 편집도 메모리에서 시험할 수 있지만 조명, 높이맵, 블록 엔티티와 주변 블록 갱신은 아직 처리하지 않습니다. 플레이어 인벤토리·장비·제작법 책의 변경은 다른 NBT 필드를 유지하는 메모리 초안으로 만들 수 있으며, 열린 메뉴나 보존할 수 없는 아이템은 거부합니다. 대표 초안을 Java 26.3의 아이템·장비·제작법 책 코덱으로 읽는 검사를 통과했습니다. 이 초안은 파일에 기록하지 않으며 실제 플레이 월드 저장에는 연결되지 않았습니다. 외부 청크를 새로 써야 하는 크기의 청크는 아직 거부하고, 정전 시 파일 시스템의 내구성은 검증하지 않았습니다.

기존 월드의 선택한 청크를 **읽기 전용 뷰어**에서 볼 수도 있습니다. 기본적으로 저장된 플레이어의 차원과 위치 주변을 열며, 플레이어 위치가 없거나 다른 차원을 보려면 청크를 지정합니다.

```powershell
cargo run -p native-client -- --view-world "D:\WorldCopies\my-world"
cargo run -p native-client -- --view-world "D:\WorldCopies\my-world" --dimension minecraft:the_nether --chunk 0 0 --radius 1
```

26.3은 기본 블록 상태를 속성 없이 저장합니다. 뷰어가 해당 속성을 복원하도록 하려면, Java 25로 로컬 설치에서 공식 블록 보고서를 한 번 준비한 뒤 뷰어에 지정합니다. 생성된 데이터는 `.local`에 보관되며 저장소에 포함되지 않습니다. 실행 중인 뷰어에는 Java가 필요하지 않습니다.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\tools\prepare-block-registry.ps1
cargo run -p native-client -- --view-world "D:\WorldCopies\my-world" --block-report .local\block-registry
```

설치 경로나 Java 경로가 다르면 준비 스크립트에 `-MinecraftDir` 또는 `-JavaHome`을 지정합니다. 보고서는 JAR과 보고서의 해시 및 데이터 버전을 검사하며, 다른 클라이언트 JAR에서 생성된 보고서는 거부합니다. 보고서가 없으면 기본 상태의 속성을 알 수 없어 일부 블록 모델이 자홍색으로 표시될 수 있습니다.

화면을 클릭해 마우스 시점을 조작하고 `WASD`로 비행합니다. `Space`와 `Shift`는 상승·하강, `Ctrl`은 빠른 이동, `Esc`는 마우스 해제입니다. 뷰어는 플레이어 물리나 블록 상호작용을 실행하지 않고 월드를 저장하지 않습니다. 물과 용암의 흐르는 표면, 텍스처 애니메이션, 물에 잠긴 블록을 표시합니다. 저장된 바이옴, 활성 데이터팩의 바이옴 정의와 리소스팩 색상표로 잔디·잎·물의 색을 계산합니다. 저장된 블록빛과 하늘빛은 따로 읽고 확인된 차원 유형의 환경 색상으로 합성합니다. 모델이나 텍스처를 읽지 못하면 원인을 콘솔에 출력하고 해당 부분에 자홍색 체크무늬를 표시합니다.

데이터팩의 저장된 우선순위와 새 팩 자동 활성화, 기능 플래그, 기본 형식의 오버레이·리소스 필터를 바이옴 색상에 적용합니다. 지원하지 않는 정규식이나 심볼릭 링크 팩은 경고하고 건너뜁니다. 데이터팩의 나머지 월드 생성·게임 규칙과 바이옴 레지스트리 전체 검증은 아직 구현되지 않았습니다. 뷰어의 조명은 정적인 낮 설정이며 시간·날씨·상태 효과, 주변광과 인라인 차원 유형 등은 아직 반영하지 않습니다. 누락된 조명 배열은 블록빛 0, 하늘이 있는 차원의 하늘빛 15로 대체합니다. 바이옴 경계의 색상 혼합과 늪지의 위치별 잔디 색은 아직 근사치이며, 유체 시뮬레이션과 부분 블록의 유체 가림, 블록 엔티티, 정확한 조명과 투명도 등도 불완전하므로 화면을 바닐라와 같다고 간주하면 안 됩니다.

## 기존 월드 실험적 플레이

복사한 26.3 월드를 대상으로 저장되지 않는 실험적 세션을 열 수 있습니다. 기본적으로 저장된 플레이어 위치에서 시작하며, `--chunk X Z`로 시작 청크를 지정할 수도 있습니다.

```powershell
cargo run -p native-client -- --play-world "D:\WorldCopies\my-world" --block-report .local\block-registry
```

화면 클릭으로 마우스를 잡고 `WASD`, 마우스, `Space`로 이동·시점·점프를 조작합니다. `Ctrl` 또는 `W`를 빠르게 두 번 눌러 달리고 `Shift`로 웅크릴 수 있습니다. 물속에서는 `Space`로 위로 헤엄치고 `Shift`로 내려가며, 잠수 중 달리면 수영 자세가 됩니다. 좌클릭은 파괴, 우클릭은 설치, `1`~`9`는 저장된 단축바 선택입니다. 현재 설치 가능한 아이템은 기본 구성의 잔디·흙·돌·돌 벽돌·조약돌과 흙길·경작지를 포함한 흙 블록, 판자 13종·작업대와 원목 계열 50종입니다. 저장된 플레이어의 게임 모드가 없으면 월드 기본 모드를 사용합니다. 생존 모드는 설치가 성공할 때 아이템을 하나 소모하고 크리에이티브는 소모하지 않습니다. 모험·관전에서는 이 기본 아이템의 블록 변경이 제한됩니다. 각 모드의 다른 능력은 아직 완전하지 않습니다. GUI에서 시작하면 `E`로 인벤토리, `Esc`로 일시정지를 열며, 개발용 명령행 모드에서 시작하면 `Esc`로 마우스를 해제합니다. 이동하면 주변의 기존 청크를 백그라운드에서 읽고 고정 틱에 반영합니다. 플레이 모드의 `--radius`는 최소 1청크이며, `0`을 지정해도 이웃을 미리 읽기 위해 1로 취급합니다. 멀어진 청크 중 수정한 것은 메모리에 보관해 되돌아왔을 때 복원합니다. **블록과 플레이어 변경은 메모리에만 존재하며 종료 시 사라집니다.** 원본 월드 파일을 저장하지 않습니다.

이 모드는 바닐라 플레이와 아직 다릅니다. 기본 정육면체, 반 블록, 계단, 울타리, 유리판, 화분 등의 충돌과 일부 식물·횃불을 포함한 선택 형상을 구현했으며, 조준선은 해당 형상의 빈 공간을 통과합니다. 기본 0.6블록 오르기 높이를 적용해 반 블록을 걸어서 올라가고 낮은 천장에서는 막힙니다. 일반 이동과 달리기의 가속·관성, 지면 마찰, 점프와 중력을 반영하며 얼음 위에서는 더 오래 미끄러집니다. 웅크리면 느리게 이동하고 발밑의 블록 가장자리에서 떨어지지 않도록 이동을 조정합니다. 물·용암에 닿는 깊이와 흐름, 기본 물속 이동과 수영 자세를 적용합니다. 저장된 유체 예약 틱과 플레이 중 주변 블록 변경으로 예약된 틱을 처리해, 공기와 완전한 고체로 이루어진 지형에서는 물과 용암이 퍼지고 줄어듭니다. 유체 틱도 메모리에만 반영됩니다.

물과 용암이 만나면 접촉 방향과 수원 상태에 따라 흑요석·조약돌·돌이 생기며, 용암·푸른 얼음·영혼 흙으로 현무암도 만들 수 있습니다. 물에 잠긴 블록과의 용암 접촉도 판정합니다. 수원 생성 여부는 26.3의 `data/minecraft/game_rules.dat`에서 읽습니다.

유체 처리는 아직 제한적입니다. 활성 데이터팩의 차원 유형에서 빠른 용암 속성을 읽어 용암 확산과 아이템·구슬의 유속에 적용합니다. 연결된 시간선의 선형·고정 키프레임은 월드 시계에 따라 적용합니다. 다른 보간 방식이나 시계·차원 유형을 읽을 수 없는 경우는 차원 이름을 기준으로 근사합니다. 부분 블록·식물·물에 잠긴 블록은 흐름 갱신에서 장애물로 취급하며, 용암의 무작위 지연과 다른 사용자 차원의 유체 특성은 아직 처리하지 않습니다. 청크 경계와 예약 틱 실행 순서도 완전한 서버와 다를 수 있습니다. 흐름을 막는 블록 태그와 면의 판정은 충돌 형상으로 근사합니다. 물속 산소와 기본 익사 피해, 저장된 호흡 효과와 장비의 산소 보너스를 처리하지만, 난수와 피해 처리는 전체 플레이어 시뮬레이션과 아직 통합되지 않았습니다. 물속 화면 효과, 용암 피해, 접촉 소리·입자와 거품 기둥의 특수 효과도 아직 없습니다.

낮은 공간에서는 자세와 시점 높이를 바꾸지만 시점 전환은 아직 부드럽게 보간하지 않습니다. 구현하지 않은 충돌·선택 형상은 정육면체로 취급하고, 아직 읽지 못했거나 생성되지 않은 청크는 이동과 조준을 막습니다. 새 청크의 생성은 지원하지 않습니다. 위치에 따라 형상이 달라지는 일부 식물도 아직 미지원입니다. 기본 달리기·점프·물속 이동의 탈진과 음식 사용 중의 감속을 적용합니다. 달리기 시야 변화, 상태 효과와 블록별 이동 특수 효과는 아직 구현하지 않았고, 블록 상호작용과 인벤토리도 단순화되어 있습니다. 기본 잔디·흙·돌·돌 벽돌·조약돌, 판자·작업대와 원목 계열은 빈손과 일곱 재질의 일반 삽·곡괭이·도끼·괭이·검으로 채굴할 때 속도, 적합한 도구 판정, 내구도를 적용합니다. 곡괭이로 돌을 캐면 조약돌을 얻어 다시 설치할 수 있으며 크리에이티브에서 일반 검으로 블록을 파괴할 수 없습니다. 다른 블록이나 미해석 도구는 기존 실험 동작처럼 즉시 파괴되며, 사용자 데이터팩의 채굴 태그 변경, 인챈트와 상태 효과는 아직 처리하지 않습니다. 기본 잔디·흙·돌·돌 벽돌·조약돌·판자·작업대와 원목 계열의 전리품은 설치된 26.3 또는 활성 데이터팩에서 읽고, 생성된 아이템은 낙하·충돌 후 주울 수 있습니다. 데이터팩의 무작위 전리품과 미해석 아이템, 아이템 합치기·물속 이동·저장된 아이템 엔티티는 아직 지원하지 않습니다. 떨어진 블록의 모델과 움직임은 근사치이며 바닐라와의 화면·물리 비교가 필요합니다. 완전한 생존·엔티티·블록 틱, 변경에 따른 조명·높이맵 갱신 및 저장은 아직 지원하지 않습니다. 메시 생성은 백그라운드에서 수행하며 변경된 청크와 경계에서 영향을 받은 이웃을 다시 만들고 나머지는 재사용합니다. 새로운 블록·바이옴 종류나 큰 기준 좌표 이동으로 전체 메시와 아틀라스를 다시 준비할 수 있습니다. 불투명 지형은 청크별 GPU 버퍼와 렌더 명령을 재사용하고, 반투명 지형은 카메라나 형상이 바뀔 때만 정렬해 업로드합니다. 반투명 정렬은 여전히 전체 표시 영역을 대상으로 수행하므로 이동 중 큰 월드에서의 프레임 시간은 별도로 측정해야 합니다.

사과·빵·익힌 고기 등 효과나 빈 용기가 없는 일반 음식 27종은 우클릭을 누르고 있으면 먹습니다. 먹는 중에는 이동이 느려지고 사용 중인 손의 아이템이 움직이며 버튼을 놓거나 다른 손 아이템으로 바꾸면 취소됩니다. 저장된 난이도와 자연 체력 회복 규칙에 따라 포만도·탈진·회복과 기본 굶주림 피해를 처리합니다. 피해 처리는 현재 기본 흡수와 체력에 한정되며 부활도 앞에서 설명한 조건에서만 지원합니다. 음식 사용 소리·입자, 상태 효과가 있는 음식, 용기 반환과 사용자 지정 음식 구성 요소는 미구현입니다.

## 검증

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

다음 선택적 테스트는 창을 열지 않고 GPU 파노라마와 메뉴 합성을 검사하며, 로컬 26.3 자산을 이용해 오프스크린 프레임 시간도 측정합니다. 기본 백엔드는 Vulkan이며 `NATIVE_TEST_GPU_BACKEND=dx12`로 DirectX 12를 선택할 수 있습니다. 이 측정에는 창 표시와 수직 동기화 시간이 포함되지 않습니다.

```powershell
cargo test --release -p native-client --bin native-client gpu_ -- --ignored --nocapture
$env:NATIVE_TEST_GPU_BACKEND = 'dx12'
cargo test --release -p native-client --bin native-client gpu_ -- --ignored --nocapture
Remove-Item Env:NATIVE_TEST_GPU_BACKEND
```

메뉴의 실제 창에서 재그리기 간격과 CPU 호출 시간을 조사하려면, 별도의 최적화된 개발 빌드에서 다음 진단을 사용할 수 있습니다. 화면마다 처음 30회를 제외한 300회에 대해 중앙값과 95·99백분위를 출력합니다. 재그리기 호출 시간에는 수직 동기화 대기가 포함될 수 있으며, 이 수치는 GPU 실행 시간이나 실제 모니터 출력 시각이 아닙니다. 다음 명령은 일반 릴리스 빌드를 덮어쓰지 않습니다.

```powershell
cargo rustc --release --target-dir .local\menu-diagnostics -p native-client --bin native-client -- -C debug-assertions=yes
$env:NATIVE_MENU_FRAME_TIMINGS = '1'
& .\.local\menu-diagnostics\release\native-client.exe
Remove-Item Env:NATIVE_MENU_FRAME_TIMINGS
```

설치된 자산과 Windows GPU를 사용하는 최적화된 테스트 실행으로도 1080p 메뉴를 측정할 수 있습니다. 이 계측 코드는 테스트 실행 파일에만 포함되며, 300개의 이벤트 간격과 렌더 스레드의 버퍼 획득·명령 인코딩·제출·표시 호출 시간을 출력합니다. 화면에 실제 표시된 시각은 측정하지 않습니다.

```powershell
$env:NATIVE_MENU_FRAME_TIMINGS = '1'
$env:NATIVE_MENU_TEST_1080 = '1'
cargo test --release -p native-client --bin native-client measure_title_window_pacing -- --ignored --nocapture
Remove-Item Env:NATIVE_MENU_FRAME_TIMINGS
Remove-Item Env:NATIVE_MENU_TEST_1080
```

로컬 바닐라 26.3 파일을 사용하는 선택적 차등 테스트는 별도로 실행합니다. NBT 읽기·쓰기, 바닐라 코드가 생성한 영역 파일과 팔레트·유체 예약 틱·수원 생성 규칙, 난수 두 계열의 기본 메서드, 모델 정점·UV·회전과 일부 바닐라 리소스, 유체의 높이·일부 흐름 계산과 기본 바이옴의 색상 계산을 확인합니다. 공기·완전한 고체·물·용암으로 구성한 지형에서 유체 한 틱의 블록 변경과 여러 용암 접촉 사례도 Java 메서드와 비교합니다. 구현된 블록 충돌·선택 형상, 일부 경계 조건의 광선 선택, 제공된 블록 형상에 대한 이동 충돌과 오르기 결과도 바닐라와 비교합니다. 걷기·달리기·웅크리기 입력의 이동 계산, 물·용암 접촉과 기본 이동, 달리기 조건, 자세 선택과 가장자리 보정은 충돌과 주변 상태를 제어한 테스트 환경에서 Java 메서드와 비교하며, 플레이어의 전체 틱과 특수 블록 효과를 검증한 것은 아닙니다. 설치된 26.3의 67개 기본 바이옴과 데이터팩 선택·우선순위 사례, 일반 제작법의 대표 배치와 결과 슬롯 조작도 비교하며 늪지 잔디의 위치별 색은 비교 대상에서 제외합니다. 테스트는 임시 디렉터리만 사용하고 게임 설치나 저장 월드를 수정하지 않습니다. 실제 플레이 월드 왕복 테스트는 아직 하지 않았습니다.

```powershell
cargo test -p native-persistence --test vanilla_nbt -- --ignored
cargo test -p native-persistence --test vanilla_chunk_edit -- --ignored
cargo test -p native-persistence --test vanilla_region_write -- --ignored
cargo test -p native-resources --test vanilla_resources -- --ignored
cargo test -p native-resources --test vanilla_block_defaults -- --ignored
cargo test -p native-resources --test vanilla_geometry -- --ignored
cargo test -p native-resources --test vanilla_biomes -- --ignored
cargo test -p native-resources --test vanilla_datapacks -- --ignored
cargo test -p native-resources --test vanilla_respawn_tags -- --ignored
cargo test -p native-resources --test vanilla_respawn_environment -- --ignored
cargo test -p native-resources --test vanilla_recipes -- --ignored
cargo test -p native-resources --test vanilla_transformers -- --ignored
cargo test -p native-resources --lib installed_vanilla_recipes_load_without_shaped_or_shapeless_parse_errors -- --ignored
cargo test -p native-world --test vanilla_fluid -- --ignored
cargo test -p native-session --test vanilla_respawn -- --ignored
cargo test -p native-world --test vanilla_fluid_contact -- --ignored
cargo test -p native-world --test vanilla_fluid_tick -- --ignored
cargo test -p native-world --test vanilla_lightmap -- --ignored
cargo test -p native-world --test vanilla_collision -- --ignored
cargo test -p native-world --test vanilla_raycast -- --ignored
cargo test -p native-world --test vanilla_movement -- --ignored
cargo test -p native-world --test vanilla_sneak -- --ignored
cargo test -p native-session --test vanilla_player_motion -- --ignored
cargo test -p native-session --test vanilla_physics -- --ignored
cargo test -p native-session --test vanilla_fluid_motion -- --ignored
cargo test -p native-session --test vanilla_sprint -- --ignored
cargo test -p native-session --test vanilla_food -- --ignored
cargo test -p native-client --bin native-client health_and_food_sprites_match_vanilla_26_3 -- --ignored
cargo test -p native-client --bin native-client survival_hud_uses_resource_pack_sprites_and_invalidates_when_needed -- --ignored
cargo test -p native-session --test vanilla_inventory -- --ignored
cargo test -p native-session --test vanilla_equipment -- --ignored
cargo test -p native-session --lib java_written_equipment_has_the_same_oxygen_modifiers -- --ignored
cargo test -p native-client --bin native-client palette_mapping_matches_java_for_transparency_and_duplicate_colors -- --ignored
cargo test -p native-client --bin native-client armor_models_use_trim_palettes_and_per_item_dye -- --ignored
cargo test -p native-client --bin native-client player_cuboids_and_uvs_match_java_models -- --ignored
cargo test -p native-client --bin native-client vanilla_equipment_layers_trims_and_resource_pack_overrides_load -- --ignored
cargo test -p native-client --bin native-client inventory_redraws_the_avatar_when_armor_finishes_loading -- --ignored
cargo test -p native-session --test vanilla_mining -- --ignored
cargo test -p native-session --lib crafting_stone_bricks_consumes_ingredients_at_tick_boundaries -- --ignored
cargo test -p native-session --lib world_data_pack_can_override_the_crafting_result -- --ignored
cargo test -p native-session --lib world_data_pack_can_override_axe_transformations -- --ignored
cargo test -p native-session --lib world_data_pack_can_override_soil_tags_and_interaction_loot -- --ignored
cargo test -p native-session --lib rooted_dirt -- --ignored
cargo test -p native-client --bin native-client reads_player_inventory_encoded_by_vanilla_26_3 -- --ignored
cargo test -p native-client --bin native-client world_data_pack_colors_fluid_vertices -- --ignored
cargo test -p native-client --test inspect viewer_renders_a_world_without_modifying_it -- --ignored
cargo test -p native-client --test inspect viewer_resolves_default_state_using_a_verified_block_report -- --ignored
cargo test -p native-client --test inspect viewer_renders_water_lava_and_waterlogged_blocks_without_modifying_world -- --ignored
cargo test -p native-client --test inspect experimental_world_play_updates_mesh_without_writing_world_files -- --ignored
```

기본 Minecraft 경로가 아니면 `MINECRAFT_DIR` 환경 변수를 설정합니다. 기본 Minecraft Launcher의 Java 25 JDK를 찾지 못하면 `MINECRAFT_JAVA_HOME` 또는 `JAVA_HOME`을 Java 25 JDK 경로로 설정합니다.

## 남은 주요 작업

기존 월드의 플레이와 안전한 저장, 전체 리소스팩·데이터팩, 실제 월드 생성, 바닐라 물리와 생존 규칙, 엔티티·레드스톤, UI·오디오 및 원본 대비 성능 측정은 아직 구현되지 않았습니다. 자동 테스트를 통과한 범위 밖의 바닐라 호환성은 보장하지 않습니다.

구현 순서와 완료 조건은 [남은 개발 계획](docs/ROADMAP.md)을 참조하세요.
