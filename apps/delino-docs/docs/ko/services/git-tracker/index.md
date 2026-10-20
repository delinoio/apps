# Git Tracker

[git-tracker.delino.io](https://git-tracker.delino.io)에서 Git 커밋의 수치 측정값을 추적하세요. Git Tracker는 무료이며 포크 풀 리퀘스트를 포함한 공개 및 비공개 GitHub 저장소를 지원합니다. 파일 크기, 실행 시간, 처리량, 개수 등 단일 수치 측정에 사용할 수 있습니다.

## 저장소 연결

1. Delino 계정으로 로그인하고 개인 또는 조직 공간을 선택합니다.
2. 공간 소유자에게 공간의 그룹을 통해 Git Tracker View, Edit 또는 Manage 권한을 부여해 달라고 요청합니다. View는 읽기를 허용합니다. Edit는 업로드와 완료를 추가로 허용합니다. Manage는 연결, 토큰 관리, 영구 삭제도 허용합니다.
3. 저장소에 Git Tracker GitHub App을 설치합니다.
4. **Manage repositories**를 엽니다. `owner/repository`와 연결된 GitHub 계정의 저장소 관리자 권한이 있는 GitHub 액세스 토큰을 입력합니다. 서비스는 GitHub 권한을 확인하며 해당 액세스 토큰을 보관하지 않습니다.
5. 저장소를 연결합니다. Delino 공간의 Owner/Admin 권한도 필요합니다. 저장소는 한 번에 하나의 공간에만 연결할 수 있습니다. Action과 API 호출에 사용할 연결 ID를 저장소 관리 화면에서 복사합니다. 연결을 해제한 후에는 권한이 있는 다른 공간에서 새 연결 ID로 연결할 수 있으며, 이전 공간에 보관된 기록은 비공개로 유지됩니다.

공개 저장소의 측정값은 로그인하지 않아도 읽을 수 있습니다. 비공개 측정값을 읽으려면 공간 멤버십과 Git Tracker 읽기 권한이 필요합니다. 공간 접근 권한은 해당 공간에 연결된 모든 저장소에 적용됩니다.

연결을 해제하거나 App을 제거하면 업로드가 중단되고 보관된 기록이 비공개로 전환됩니다. 권한이 있는 멤버는 계속 읽을 수 있습니다. 공개에서 비공개로의 변경이 확인되면 과거 데이터도 비공개가 됩니다. GitHub에서 현재 공개 상태를 확인할 수 없으면 동기화가 성공할 때까지 마지막으로 확인된 상태를 유지합니다.

## 데이터셋 정의

저장소에 YAML 설정을 보관합니다. ID는 유지하고 이름만 변경하면 기록이 보존됩니다. 한 행의 모든 열은 함께 업로드해야 합니다.

```yaml
id: file-size
name: JavaScript file sizes
columns:
  - id: original
    name: Original size
    unit: B
    direction: lower
    absoluteThreshold: 1024
  - id: minified
    name: Minified size
    unit: B
    direction: lower
    percentageThreshold: 1
  - id: gzip
    name: Gzipped size
    unit: B
    direction: lower
    absoluteThreshold: 128
    percentageThreshold: 2
```

JSON 행을 업로드합니다. 다음은 SWC 스타일의 파일 크기 표 예시입니다.

```json
{
  "rows": [
    { "id": "jquery", "name": "jQuery", "values": { "original": 285314, "minified": 87533, "gzip": 30321 } },
    { "id": "three", "name": "Three.js", "values": { "original": 1321000, "minified": 675000, "gzip": 170000 } }
  ]
}
```

Action은 이진 부동소수점 변환 없이 JSON 십진수 표기를 보존합니다. Connect를 직접 호출할 때는 `"9007199254740993.0000000000000001"`처럼 십진수 값을 문자열로 전송합니다. 음수와 유한 소수를 지원합니다. 표시를 위한 반올림은 회귀 판단에 영향을 주지 않습니다.

지원 단위는 `B`, `kB`, `MB`, `GB`, `KiB`, `MiB`, `GiB`; `ns`, `μs`, `ms`, `s`; `count`, `%`, `unitless`; `ops/s`, `B/s`, `kB/s`, `MB/s`, `GB/s`, `KiB/s`, `MiB/s`, `GiB/s`입니다. 호환되는 단위는 비교 전에 변환합니다. 종류가 호환되지 않으면 비교할 수 없습니다. 예를 들어 1048576 B는 1 MiB로 표시하며, 정확한 값은 표에서 확인할 수 있습니다.

개선 방향은 `lower`, `higher`, `neutral`입니다. 중립 열에서는 회귀가 발생하지 않습니다. 임계값은 해당 열의 모든 행에 적용됩니다. 절대 임계값과 백분율 임계값이 모두 있으면 둘 중 하나라도 초과할 때 회귀이며, 같은 값은 초과가 아닙니다. 임계값이 없으면 변화는 표시하지만 회귀로 실패하지 않습니다. 악화되는 방향의 변화만 판단합니다.

백분율 변화는 `(new - baseline) / abs(baseline) * 100`입니다. 기준값이 0이면 백분율 변화를 계산할 수 없습니다. 절대 임계값이 있으면 판단할 수 있지만 백분율 규칙만 있으면 판단을 보류합니다. 비교에는 이후 설정이 아닌 대상 실행에 저장된 설정을 사용합니다.

## 공식 GitHub Action 사용

`delinoio/apps/actions/git-tracker`를 검토한 전체 커밋 SHA에 고정합니다. 아래 자리표시자를 Git Tracker 연결 ID, 체크아웃 리비전, Action 리비전으로 바꿉니다. OIDC를 사용하려면 `id-token: write`를 부여합니다. 저장소 업로드 토큰을 대신 사용할 수도 있습니다. GitHub 시크릿에 저장하고 `token`으로 전달합니다.

```yaml
permissions:
  contents: read
  id-token: write
jobs:
  measure:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@<reviewed-checkout-commit-sha>
      - run: ./measure.sh
      - uses: delinoio/apps/actions/git-tracker@<reviewed-action-commit-sha>
        with:
          repository-id: '019b0000-0000-7000-8000-000000000001'
          configuration: measurements.yaml
          measurements: measurements.json
          fail-on-regression: 'false'
  mark-as-done:
    needs: measure
    if: always()
    runs-on: ubuntu-latest
    steps:
      - uses: delinoio/apps/actions/git-tracker@<reviewed-action-commit-sha>
        with:
          operation: complete
          repository-id: '019b0000-0000-7000-8000-000000000001'
```

독립 작업은 기본 실행 ID인 `repository_id/run_id/run_attempt`를 공유하며 서로 다른 행을 업로드할 수 있습니다. 재실행에는 새 실행 ID를 사용합니다. Actions OIDC에는 기본 ID가 필요하며, 사용자 지정 실행 ID는 저장소 토큰을 사용하는 호출자용입니다. 포크 OIDC 업로드는 PR 헤드로 제한되며, 기준을 재정의하거나 업스트림 푸시 기록을 작성할 수 없습니다.

각 업로드 작업은 자신이 업로드한 행만 판단합니다. 기본적으로 회귀를 보고하되 CI를 실패시키지 않습니다. 회귀 시 해당 작업을 실패시키려면 `fail-on-regression: 'true'`를 설정합니다. 비교값이 없으면 CI를 실패시키지 않습니다. 완료 작업은 측정이 끝났다는 사실만 나타냅니다. CI 성공·실패·취소를 제출하거나 전체 회귀를 집계해 통과 여부를 판단하지 않습니다. 모든 작업이 생략된 경우 빈 실행을 완료할 수 있습니다.

네트워크 오류와 일시적인 API 실패는 최대 세 번 재시도합니다. 재시도 후에는 기본적으로 작업이 실패합니다. 대신 경고하고 통과시키려면 `warn-on-api-error: 'true'`를 설정합니다. PR 댓글 실패는 경고만 남기며, 저장된 데이터를 무효화하거나 그 자체로 CI를 실패시키지 않습니다. Action은 상세 링크를 로그에 남기고 Job Summary를 작성합니다.

PR 댓글 하나가 PR의 최신 커밋에서 가장 최근에 등록된 실행의 누적 결과를 반영합니다. 이전 실행은 웹 기록에서 계속 확인할 수 있습니다.

## Connect API 호출

API 원본 주소는 `https://git-tracker.api.delino.io`입니다. Connect JSON 요청에는 `Content-Type: application/json`과 `Connect-Protocol-Version: 1`을 사용합니다. 관리자는 저장소 관리 화면에서 미래의 만료일이 필수인 저장소 토큰을 발급하거나 폐기할 수 있습니다.

```sh
curl --fail-with-body \
  'https://git-tracker.api.delino.io/gittracker.v1.GitTrackerService/Upload' \
  -H "Authorization: Bearer ${GIT_TRACKER_TOKEN}" \
  -H 'Content-Type: application/json' \
  -H 'Connect-Protocol-Version: 1' \
  --data-binary @upload.json
```

`upload.json`에는 `context`, `dataset`, `rows`가 포함됩니다.

```json
{
  "context": {
    "repositoryId": "019b0000-0000-7000-8000-000000000001",
    "executionId": "my-historical-run",
    "commit": "0123456789012345678901234567890123456789",
    "branch": "main"
  },
  "dataset": {
    "id": "size",
    "name": "Size",
    "columns": [{ "id": "bytes", "name": "Bytes", "unit": "B", "direction": "DIRECTION_LOWER", "absoluteThreshold": "1024" }]
  },
  "rows": [{ "id": "bundle", "name": "Bundle", "values": { "bytes": "1048576" } }]
}
```

같은 `context`로 `/gittracker.v1.GitTrackerService/Complete`를 호출합니다. PR 업로드에는 `pullRequest`를 포함하고 PR의 기준 브랜치와 헤드 커밋을 사용합니다. `baselineCommit`으로 명시적인 전체 기준 SHA를 선택합니다. 과거 커밋에도 같은 업로드 API를 사용합니다. 전송 전에 십진수 문자열을 JavaScript `Number`로 변환하지 마세요.

## 실행 및 비교 규칙

첫 업로드가 해당 실행의 각 데이터셋 설정을 고정합니다. 이후 다른 설정은 거부합니다. 기존 행을 동일한 수치 값으로 반복 전송하면 멱등적으로 처리합니다. 충돌하는 값은 거부합니다. 배치는 원자적이므로 유효하지 않거나 충돌하는 행이 있으면 해당 요청의 어떤 행도 저장하지 않습니다.

데이터는 즉시 **Measuring** 상태로 표시됩니다. 완료된 후에만 대표 결과가 됩니다. 커밋의 대표 결과는 회귀 여부와 관계없이 가장 최근에 등록된 완료 실행입니다. 이전 실행이 늦게 완료되어도 더 최근에 등록된 완료 실행을 대체하지 않습니다. 완료를 반복 호출해도 완료 시각과 순서는 바뀌지 않습니다.

새 측정 데이터가 저장되지 않은 채 6시간이 지나면 미완료 실행은 **Interrupted** 상태가 됩니다. 동일한 재전송은 이 시간을 초기화하지 않습니다. 완료되거나 중단된 실행은 추가 업로드를 거부하며, 중단된 실행은 완료도 거부합니다. 대신 새 실행을 시작합니다. 데이터는 삭제할 때까지 유지됩니다.

자동 PR 비교는 첫 업로드에서 merge-base 커밋과 해당 커밋의 대표 완료 실행을 고정합니다. 기준이 없으면 나중에 업로드되어도 계속 사용할 수 없는 상태로 남습니다. 자동 푸시 비교는 행별로 첫 번째 부모 계보를 탐색하여 첫 업로드 시점까지 완료된 가장 가까운 측정 조상을 사용합니다. 이후 작업도 같은 기준 시점을 사용합니다. 표에는 실제로 사용한 각 소스 커밋과 실행이 표시됩니다. 명시적 기준 SHA 비교에는 행별 조상 대체를 사용하지 않습니다.

웹사이트의 두 커밋 비교는 선택한 커밋의 대표 완료 실행만 사용합니다. 새 행·열과 누락되거나 호환되지 않는 데이터는 비교할 수 없으며 회귀 실패를 일으키지 않습니다. 삭제된 기준 데이터는 복원하지 않습니다. 완료 전의 없는 행은 미수집 상태이고, 완료 후에는 누락 상태입니다.

그래프는 첫 번째 부모 순서에 따라 커밋을 같은 간격으로 표시합니다. 생략된 커밋을 가로질러 실제 측정 지점을 연결하며 값을 이월하거나 만들어 내지 않습니다. 데이터셋, 브랜치, 열, 항목, 날짜, 커밋 범위로 필터링할 수 있습니다. 정확한 값과 키보드로 접근할 수 있는 실행 링크는 데이터 표를 사용하세요.

## 삭제 및 문제 해결

관리자는 확인 후 저장소의 측정값 전체, 데이터셋 또는 실행 하나를 영구 삭제할 수 있습니다. 기준이 삭제된 비교는 사용할 수 없게 됩니다. Delcore가 공간 삭제를 확정하면 Git Tracker는 해당 공간의 연결, 토큰, 설정, 기록을 삭제합니다.

권한 오류가 발생하면 선택한 공간과 현재 Git Tracker 역할을 확인합니다. 업로드 충돌이 발생하면 실행 ID, 데이터셋 설정, 완전한 행 값을 확인합니다. 중단되거나 완료된 실행에는 새 실행 ID를 사용합니다. 네트워크·서버 오류는 재시도하고, 도움을 요청할 때 응답의 상관관계 ID를 보관합니다. 공개 보고에 토큰, 자격 증명, 비공개 측정값을 공유하지 마세요.

PR 실행 측정값은 계속 확인할 수 있지만 업스트림 브랜치의 그래프 지점이나 자동 기준으로 사용하지 않습니다. 따라서 포크 측정값이 같은 커밋 SHA의 기준 브랜치 데이터나 설정을 대체하지 못합니다.
