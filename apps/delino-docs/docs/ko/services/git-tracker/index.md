# Git Tracker

[git-tracker.delino.io](https://git-tracker.delino.io)에서 Git 커밋의 수치 측정값을 추적하세요. Git Tracker는 무료입니다. 포크의 풀 리퀘스트를 포함하여 공개 및 비공개 GitHub 저장소를 지원합니다. 파일 크기, 소요 시간, 처리량, 개수 및 기타 스칼라 측정값에 사용할 수 있습니다.

## 저장소 연결

1. Delino 계정으로 로그인하고 개인 또는 조직 스페이스를 선택합니다.
2. 스페이스 소유자에게 스페이스의 그룹을 통해 Git Tracker 보기, 편집 또는 관리 권한을 부여해 달라고 요청합니다. 보기 권한은 읽기를 허용합니다. 편집 권한에는 업로드와 완료 처리가 추가됩니다. 관리 권한에는 연결, 토큰 관리 및 영구 삭제가 추가됩니다.
3. 저장소에 Git Tracker GitHub App을 설치합니다.
4. **저장소 관리**를 엽니다. `owner/repository`와 저장소 관리자 액세스 권한이 있는 연결된 GitHub 계정의 GitHub 액세스 토큰을 입력합니다. 서비스는 GitHub 권한을 확인하며 해당 액세스 토큰을 보관하지 않습니다.
5. 저장소를 연결합니다. Delino 스페이스 소유자/관리자 권한도 필요합니다. 저장소는 한 번에 하나의 스페이스에만 연결할 수 있습니다. Action 및 API 호출에 사용할 연결 ID를 저장소 관리에서 복사합니다. 연결을 해제하면 권한이 있는 다른 스페이스에서 새 연결 ID로 연결할 수 있으며, 이전 스페이스에 보관된 기록은 비공개로 유지됩니다.

공개 저장소의 측정값은 로그인하지 않고도 읽을 수 있습니다. 비공개 측정값을 읽으려면 스페이스 멤버십과 Git Tracker 읽기 권한이 필요합니다. 스페이스 액세스 권한은 해당 스페이스에 연결된 모든 저장소에 적용됩니다.

연결을 해제하거나 App을 제거하면 업로드가 중단되고 보관된 기록이 비공개로 전환됩니다. 권한이 있는 멤버는 계속 읽을 수 있습니다. 공개에서 비공개로 변경된 것이 확인되면 과거 데이터도 비공개로 전환됩니다. GitHub에서 현재 공개 여부를 확인할 수 없는 경우, Git Tracker는 동기화에 성공할 때까지 마지막으로 확인된 공개 여부를 유지합니다.

## 데이터셋 정의

저장소에 YAML 설정을 보관하세요. ID는 고정되며, 이름은 기록을 잃지 않고 변경할 수 있습니다. 한 행의 모든 열은 함께 업로드해야 합니다.

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

예를 들어 SWC 스타일의 파일 크기 표와 같은 JSON 행을 업로드합니다.

```json
{
  "rows": [
    { "id": "jquery", "name": "jQuery", "values": { "original": 285314, "minified": 87533, "gzip": 30321 } },
    { "id": "three", "name": "Three.js", "values": { "original": 1321000, "minified": 675000, "gzip": 170000 } }
  ]
}
```

Action은 이진 부동소수점 변환 없이 JSON의 십진수 표기를 보존합니다. Connect를 직접 호출하는 클라이언트는 `"9007199254740993.0000000000000001"`과 같이 십진수 값을 문자열로 전송합니다. 음수와 유한소수가 지원됩니다. 표시를 위한 반올림은 성능 저하 판정에 영향을 주지 않습니다.

지원되는 단위는 `B`, `kB`, `MB`, `GB`, `KiB`, `MiB`, `GiB`, `ns`, `μs`, `ms`, `s`, `count`, `%`, `unitless`, 그리고 `ops/s`, `B/s`, `kB/s`, `MB/s`, `GB/s`, `KiB/s`, `MiB/s`, `GiB/s`입니다. 호환되는 단위는 비교 전에 변환됩니다. 호환되지 않는 종류는 비교할 수 없습니다. 예를 들어 1048576 B는 1 MiB로 표시되며, 정확한 값은 표에서 계속 확인할 수 있습니다.

방향은 `lower`, `higher`, `neutral`입니다. 중립 열에서는 성능 저하가 발생하지 않습니다. 임계값은 해당 열의 모든 행에 적용됩니다. 절대 임계값과 백분율 임계값이 모두 있는 경우, 둘 중 하나라도 초과하면 성능 저하로 판정하며, 같은 값은 초과로 보지 않습니다. 임계값이 없으면 변경 사항은 표시되지만 성능 저하로 인한 실패는 발생하지 않습니다. 악화되는 방향의 변경만 판정합니다.

변화율은 `(new - baseline) / abs(baseline) * 100`입니다. 기준값이 0이면 변화율이 없습니다. 이 경우에도 절대 임계값으로는 판정할 수 있지만, 백분율만 사용하는 규칙은 판정을 유보합니다. 비교에는 이후의 설정이 아니라 대상 실행에 저장된 설정을 사용합니다.

## 공식 GitHub Action 사용

`delinoio/apps/actions/git-tracker`를 검토된 전체 커밋 SHA로 고정하세요. 아래 플레이스홀더를 Git Tracker 연결 ID, 체크아웃 리비전, Action 리비전으로 바꾸세요. OIDC를 위해 `id-token: write`를 부여하세요. 저장소 업로드 토큰을 대신 사용할 수도 있습니다. GitHub 시크릿에 저장하고 `token`으로 전달하세요.

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

독립적인 작업은 기본 실행 ID인 `repository_id/run_id/run_attempt`를 공유하며, 서로 겹치지 않는 행을 업로드할 수 있습니다. 재실행에는 새 실행 ID를 사용하세요. Actions OIDC에는 기본 식별자가 필요하며, 사용자 지정 실행 ID는 저장소 토큰을 사용하는 호출자용입니다. 동일 저장소의 PR 작업은 `baseline`을 설정할 수 있습니다. 포크 OIDC 업로드는 PR 헤드로 제한되며, 기준값 재정의를 제공하거나 업스트림 푸시 기록에 쓸 수 없습니다.

각 업로드 작업은 자신이 업로드한 행만 판정합니다. 기본적으로 성능 저하는 CI를 실패시키지 않고 보고됩니다. 성능 저하가 발생했을 때 해당 작업을 실패시키려면 `fail-on-regression: 'true'`를 설정하세요. 비교 결과가 없어도 CI는 실패하지 않습니다. 완료 작업은 측정이 끝났다는 것만 알리며, CI 성공/실패/취소 상태를 제출하거나 집계 결과에 따른 통과 여부를 판정하지 않습니다. 모든 작업을 건너뛴 경우에는 빈 완료 처리도 유효합니다.

네트워크 오류와 일시적인 API 오류는 최대 세 번 재시도합니다. 기본적으로 재시도 후에도 오류가 지속되면 작업이 실패합니다. 대신 경고를 표시하고 통과시키려면 `warn-on-api-error: 'true'`를 설정하세요. PR 댓글 실패는 경고만 표시하며, 저장된 데이터를 무효화하거나 그 자체로 CI를 실패시키지 않습니다. Action은 상세 정보 링크를 로그에 남기고 작업 요약을 작성합니다.

하나의 PR 댓글에 PR의 최신 커밋에서 가장 최근에 등록된 실행의 누적 결과가 반영됩니다. 이전 실행은 웹 기록에서 계속 확인할 수 있습니다.

## Connect API 호출

API 오리진은 `https://git-tracker.api.delino.io`입니다. Connect JSON 요청은 `Content-Type: application/json`과 `Connect-Protocol-Version: 1`을 사용합니다. 관리자는 미래의 만료 시점을 반드시 지정하여 저장소 토큰을 발급하고, 저장소 관리에서 폐기할 수 있습니다.

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

동일한 `context`로 `/gittracker.v1.GitTrackerService/Complete`를 호출합니다. PR 업로드에는 `pullRequest`를 포함하고 PR의 베이스 브랜치와 헤드 커밋을 사용하세요. `baselineCommit`은 명시적인 전체 기준 SHA를 선택합니다. 과거 커밋에도 동일한 업로드 API를 사용합니다. 십진수 문자열을 전송하기 전에 JavaScript `Number`를 거쳐 변환하지 마세요.

## 실행 및 비교 규칙

첫 업로드 시 해당 실행에 대한 각 데이터셋의 설정이 고정됩니다. 이후 다른 설정은 거부됩니다. 기존 행을 동일한 수치 값으로 반복 전송하는 것은 멱등적입니다. 충돌하는 값은 거부됩니다. 배치는 원자적으로 처리됩니다. 유효하지 않거나 충돌하는 행이 있으면 해당 요청의 데이터는 아무것도 저장되지 않습니다.

데이터는 즉시 **측정 중** 상태로 표시됩니다. 완료된 후에만 대표 결과가 됩니다. 커밋의 대표 결과는 성능 저하가 있는 실행을 포함하여, 등록 시점이 가장 최근인 완료된 실행입니다. 이전 실행이 늦게 끝나더라도 더 나중에 등록되어 완료된 실행을 대체하지 않습니다. 완료 처리를 반복해도 완료 시간이나 순서는 바뀌지 않습니다.

새로 저장된 측정 데이터 없이 6시간이 지나면 미완료 실행은 **중단됨** 상태가 됩니다. 동일한 데이터의 재전송은 이 타이머를 초기화하지 않습니다. 완료되거나 중단된 실행은 추가 업로드를 거부하며, 중단된 실행은 완료 처리도 거부합니다. 대신 새 실행을 시작하세요. 데이터는 삭제할 때까지 유지됩니다.

자동 PR 비교는 첫 업로드 시 병합 기준 커밋과 그 커밋의 대표 완료 실행을 고정합니다. 기준 데이터가 없으면 나중에 업로드하더라도 계속 사용할 수 없습니다. 자동 푸시 비교는 행마다 첫 번째 부모 계보를 개별적으로 탐색하고, 첫 업로드 기준 시점까지 완료된 측정값이 있는 가장 가까운 조상 커밋을 사용합니다. 이후 작업도 동일한 기준 시점을 사용합니다. 표에는 실제로 사용된 각 원본 커밋과 실행이 표시됩니다. 명시적 기준 SHA 비교는 행별 조상 대체를 사용하지 않습니다.

웹사이트의 두 커밋 비교는 선택한 커밋의 대표 완료 실행만 사용합니다. 새로운 행/열과 누락되거나 호환되지 않는 데이터는 비교할 수 없으며 성능 저하로 인한 실패를 발생시키지 않습니다. 삭제된 기준 데이터는 재구성되지 않습니다. 완료 전에는 존재하지 않는 행을 미수집으로 간주하고, 완료 후에는 누락으로 간주합니다.

그래프는 첫 번째 부모 순서에 따라 커밋을 동일한 간격으로 배치합니다. 건너뛴 커밋이 있어도 실제 측정 지점끼리 연결하며, 이전 값을 그대로 이어 쓰거나 값을 만들어 내지 않습니다. 데이터셋, 브랜치, 열, 항목, 날짜 및 커밋 범위로 필터링할 수 있습니다. 정확한 값과 키보드로 접근 가능한 실행 링크는 데이터 표에서 확인하세요.

## 삭제 및 문제 해결

관리자는 확인 후 저장소의 측정값, 데이터셋 또는 개별 실행을 영구 삭제할 수 있습니다. 기준 데이터가 삭제된 비교는 사용할 수 없게 됩니다. Delcore가 스페이스 삭제를 최종 완료하면 Git Tracker는 해당 스페이스의 연결, 토큰, 설정 및 기록을 삭제합니다.

권한 오류가 발생하면 선택한 스페이스와 현재 Git Tracker 역할을 확인하세요. 업로드 충돌이 발생하면 실행 식별자, 데이터셋 설정 및 전체 행 값을 확인하세요. 중단되거나 완료된 실행에는 새 실행 ID를 사용하세요. 네트워크/서버 오류가 발생하면 재시도하고, 도움을 요청할 때 응답 상관관계 ID를 보관하세요. 공개 보고에 토큰, 자격 증명 또는 비공개 측정값을 공유하지 마세요.

PR 실행 측정값은 계속 살펴볼 수 있지만, 업스트림 브랜치의 그래프 지점이나 자동 기준 데이터로 사용되지는 않습니다. 이렇게 하면 포크의 측정값이 공유 커밋 SHA에 대한 기준 브랜치의 데이터나 설정을 대체하는 것을 방지할 수 있습니다.
