# DeliTools

DeliTools는 Connect API로 개발자를 위한 AI 도구를 제공합니다. 버전 1에는 동기 방식 메서드가 두 가지 있습니다. 텍스트 배열의 **번역**과, 텍스트에서 사용자가 정의한 스키마를 따르는 JSON으로 **데이터를 추출**하는 기능입니다. 각 호출은 같은 응답에서 완료된 결과나 오류를 반환합니다. 상태를 폴링할 작업이 없고, 나중에 가져올 결과도 없습니다. 모든 메서드는 `delitools.v1.DeliToolsService`에 있습니다.

서버 측 코드나 명령줄에서 아무 Connect 클라이언트나 HTTP의 일반 JSON으로 DeliTools를 호출하세요. API 키를 프런트엔드 코드에 넣지 마세요. `DELITOOLS_URL`을 DeliTools API 서버의 기본 URL로 설정하세요.

## 빠른 시작

1. **자격 증명을 선택합니다.** Delino 로그인 토큰을 사용하거나 `CreateKey`로 API 키를 만듭니다. 인증과 API 키 섹션을 참고하세요.
2. **테넌트 ID를 확인합니다.** 사용량은 `tenantId`에 지정한 Delino 테넌트에 청구됩니다.
3. **메서드를 호출합니다.** `Authorization: Bearer <credential>` 헤더를 함께 보냅니다.

```sh
# Translate two texts into Korean. Detect the input language.
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/TranslateTextArray" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"tenantId":"<tenant-id>","outputLanguage":"ko","model":"<allowed-model>","texts":["Good morning","Where is the station?"]}'
```

```json
{
  "requestId": "019a0000-0000-7000-8000-000000000001",
  "texts": ["좋은 아침이에요", "역이 어디예요?"]
}
```

JSON에서 필드 이름은 lowerCamelCase를 쓰고, 열거형 값은 전체 이름을 씁니다.

## 텍스트 번역

`TranslateTextArray`는 배열의 모든 문자열을 번역합니다.

| 필드             | 필수   | 의미                                                                                              |
| ---------------- | ------ | ------------------------------------------------------------------------------------------------- |
| `tenantId`       | 예     | 사용량 요금을 받는 테넌트입니다. 소문자에 하이픈을 사용하는 정규 형식의 UUID 버전 7을 사용합니다. |
| `outputLanguage` | 예     | 번역문의 BCP 47 언어 태그입니다. 예: `ko`, `pt-BR`, `zh-Hans`.                                    |
| `inputLanguage`  | 아니요 | 원문의 BCP 47 언어 태그입니다. 생략하면 배열 전체에 대해 하나의 언어를 감지합니다.                |
| `model`          | 예     | 운영자의 허용 목록에 있는 모델입니다.                                                             |
| `texts`          | 예     | 번역할 문자열입니다.                                                                              |
| `idempotencyKey` | 아니요 | 멱등성을 참고하세요.                                                                              |

응답에는 `requestId`와 `texts`가 포함되며, 번역문은 원래 순서와 원래 길이를 유지합니다.

규칙:

- 언어 값은 BCP 47 태그입니다. `outputLanguage`는 필수입니다. 올바른 BCP 47이 아니거나 ASCII가 아니거나 64바이트를 넘는 태그는 invalid-input 오류를 반환합니다.
- `texts`는 비어 있으면 안 되며, 빈 문자열이나 공백만 있는 문자열도 허용되지 않습니다.
- `inputLanguage`를 지정했는데 `outputLanguage`와 같으면 요청이 올바르지 않습니다.
- `inputLanguage`를 생략하면 DeliTools가 배열 전체에 대해 하나의 입력 언어를 감지합니다. 여러 언어가 섞인 배열도 번역됩니다. 감지된 언어가 `outputLanguage`와 같더라도 모델은 각 항목을 처리합니다. DeliTools는 원래 배열을 처리하지 않은 채 반환하지 않습니다.
- 운영자의 허용 목록에 있는 모델만 허용됩니다. 다른 모델은 invalid-model 오류를 반환합니다. Vercel AI Gateway 카탈로그에 있는 모델이라고 해서 자동으로 허용되지는 않습니다.
- 모든 항목이 성공해야만 호출이 성공합니다. 한 항목이 실패하면 전체 요청이 실패합니다. DeliTools는 일부 번역만 반환하지 않으며, 실패한 항목을 원문으로 대체하지도 않습니다.

## 데이터 추출

`ExtractDataFromText`는 텍스트를 읽고 JSON 스키마를 따르는 JSON을 반환합니다. 서비스는 고정 모델 `gpt-6-luna`(Gateway 모델 ID는 `openai/gpt-6-luna`)를 사용합니다. 모델은 선택할 수 없습니다.

| 필드             | 필수   | 의미                                                                                              |
| ---------------- | ------ | ------------------------------------------------------------------------------------------------- |
| `tenantId`       | 예     | 사용량 요금을 받는 테넌트입니다. 소문자에 하이픈을 사용하는 정규 형식의 UUID 버전 7을 사용합니다. |
| `text`           | 예     | 원본 텍스트입니다. 비어 있거나 공백만 있으면 안 됩니다.                                           |
| `jsonSchema`     | 예     | 지원하는 하위 집합을 사용하는 문자열 형태의 JSON 스키마입니다.                                    |
| `idempotencyKey` | 아니요 | 멱등성을 참고하세요.                                                                              |

응답에는 `requestId`와 `jsonData`가 포함되며, 이 값은 DeliTools가 사용자의 스키마로 검증한 JSON 문자열입니다.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/ExtractDataFromText" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"tenantId":"<tenant-id>","text":"Ada Lovelace lives in London.","jsonSchema":"{\"type\":\"object\",\"properties\":{\"name\":{\"type\":\"string\"},\"city\":{\"type\":\"string\"}},\"required\":[\"name\",\"city\"],\"additionalProperties\":false}"}'
```

```json
{
  "requestId": "019a0000-0000-7000-8000-000000000002",
  "jsonData": "{\"name\":\"Ada Lovelace\",\"city\":\"London\"}"
}
```

추출 동작 방식:

- 모델은 텍스트에 명시된 정보만 사용하도록 지시받습니다. 스키마가 요구하는 정보가 텍스트에 없으면 호출은 extraction-impossible 오류로 실패합니다. 서비스는 스키마를 만족시키려고 값을 지어내지 않습니다.
- DeliTools는 서버에서 모델 출력이 사용자의 스키마에 맞는지 확인합니다. 형식이 잘못되었거나 스키마에 맞지 않는 출력은 invalid-output 오류와 함께 실패합니다. 이런 출력은 결과로 반환되지 않으며, DeliTools는 재시도하거나 수정하지 않습니다.
- 스키마 검사와 프롬프트가 모든 값이 의미상 정확하다는 것을 보장하지는 않습니다. 추출된 데이터를 신뢰하기 전에 검토하세요. DeliTools는 각 값에 대한 근거를 반환하지 않습니다.
- DeliTools는 AI 모델을 호출하기 전에 스키마를 확인합니다. 지원하는 하위 집합을 벗어난 스키마는 AI 호출과 요금 없이 invalid-schema 오류를 반환합니다.

### 스키마 하위 집합

DeliTools는 추출 모델의 구조화된 출력이 지원하는 JSON Schema의 일부만 허용합니다.

지원:

- 루트의 객체.
- 타입 `object`, `array`, `string`, `number`, `integer`, `boolean`, `null`. 중첩은 최대 8단계입니다. 각 스키마 노드에는 문자열로 쓴 `type`이 정확히 하나만 있어야 합니다. `["string", "null"]` 같은 타입 배열은 거부됩니다.
- `properties`. 모든 속성이 `required`에 나열되어야 합니다. 속성 이름은 1~256자입니다. 모든 객체에는 비어 있더라도 `properties` 객체와 `required` 배열이 모두 있어야 합니다.
- 모든 객체에서 `additionalProperties`를 `false`로 설정.
- 배열의 `items`. 모든 배열 스키마에는 `items` 스키마가 있어야 하며, `items`가 없는 배열은 거부됩니다.
- 모든 스키마의 `description`.
- 스칼라 값을 가진 `enum`. enum은 비어 있으면 안 되고, 값은 서로 달라야 하며 노드의 `type`과 일치해야 합니다. `integer` enum에는 정수만 쓸 수 있습니다.

지원하지 않으며 실행 전에 거부됨:

- 위 목록에 없는 모든 키워드. 예: `pattern`, `minimum`, `format`, `oneOf`.
- 선택 속성. 객체의 모든 속성이 `required`에 있어야 합니다. `required`에는 각 속성을 정확히 한 번씩 나열해야 하며, `properties`에 정의되지 않은 속성을 넣을 수 없습니다.
- `additionalProperties`를 `true`로 설정했거나 누락.
- 참조: `$ref`와 재귀 스키마.
- 스키마의 JSON 객체에 있는 중복된 멤버 이름. `jsonSchema`는 중복 키가 없는 유효한 JSON 문서 하나여야 합니다.
- 제공자 제한을 넘는 스키마: 속성 총 4,998개, enum 값 998개, 또는 속성 이름과 enum 값을 합쳐 119,971자. 제공자는 총 5,000개의 속성과 120,000자를 허용하며, 나머지는 DeliTools가 응답 엔벨로프용으로 예약합니다. 값이 250개를 넘는 enum은 문자열 값을 최대 15,000자까지만 가질 수 있습니다.

지원하는 스키마:

```json
{
  "type": "object",
  "properties": {
    "invoiceNumber": { "type": "string", "description": "The invoice number" },
    "total": { "type": "number" },
    "status": { "type": "string", "enum": ["paid", "unpaid"] },
    "lines": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": { "item": { "type": "string" }, "quantity": { "type": "integer" } },
        "required": ["item", "quantity"],
        "additionalProperties": false
      }
    }
  },
  "required": ["invoiceNumber", "total", "status", "lines"],
  "additionalProperties": false
}
```

`note`가 선택 속성이고 `pattern`을 지원하지 않으므로 지원하지 않는 스키마:

```json
{
  "type": "object",
  "properties": {
    "code": { "type": "string", "pattern": "^[A-Z]{3}$" },
    "note": { "type": "string" }
  },
  "required": ["code"],
  "additionalProperties": false
}
```

## 요청 동작

- 두 메서드 모두 동기 방식입니다. 결과나 오류를 받을 때까지 연결을 열어 두세요.
- 모든 성공과 모든 오류에는 UUID 버전 7인 요청 ID가 있습니다. 지원을 요청할 때 함께 알려 주세요. 성공 응답에는 결과와 요청 ID만 들어 있습니다. 모델, 토큰 수, 요금은 포함되지 않습니다. 사용량을 확인하려면 요금 청구를 참고하세요.
- DeliTools는 운영자가 정한 한도까지 일시적인 AI 제공자 오류를 재시도합니다. 형식이 잘못된 출력은 재시도하지 않으며, 스스로 다른 모델로 전환하지도 않습니다.
- 호출을 취소하거나 연결을 끊으면 DeliTools는 AI 호출을 취소하려 시도하고 추가 재시도를 중단합니다. 이미 발생한 사용량은 계속 측정되어 청구됩니다.
- 운영자는 입력 크기, 번역 항목 수, 실행 제한 시간, 테넌트당 요청 속도, 테넌트당 동시 요청 수의 제한을 설정합니다. 이는 배포 설정이며 고정된 제품 제한이 아닙니다. 제한을 초과한 요청은 limit 오류를 반환합니다. 실행 제한 시간은 예외입니다. 이를 넘긴 호출은 timeout 오류(`ERROR_REASON_TIMEOUT`)를 반환합니다. 입력 크기 제한은 문자 수가 아니라 UTF-8 바이트 수로 계산합니다. 번역에서는 모든 `texts`의 합계이고, 추출에서는 `text`와 원본 `jsonSchema` 문자열의 합계입니다.

## 인증과 API 키

각 요청에 `Authorization: Bearer <credential>`을 보냅니다. 두 도구 모두 두 가지 자격 증명 유형을 사용할 수 있습니다.

- **Delino 로그인 토큰**(Logto 사용자 액세스 토큰)은 로그인한 사용자로서 동작합니다.
- **DeliTools API 키**는 키 종류에 정해진 대로 동작합니다.

모든 요청은 Delcore에 있는 테넌트에 대해서도 검사합니다. `TranslateTextArray`와 `ExtractDataFromText`에서는 테넌트가 활성 상태이고 유료 작업을 실행할 수 있어야 하며, 요금 청구가 이를 허용해야 합니다. DeliTools가 이 검사를 끝내지 못하면 새 유료 실행을 거부합니다. 키 관리(`CreateKey`, `ListKeys`, `RevokeKey`)는 유료 작업이 아닙니다. 현재 멤버십 또는 Owner/Admin 역할만 필요하므로, 요금 청구가 차단되어도 키를 나열하거나 폐기할 수 있습니다. DeliTools가 이 검사를 끝내지 못하면 호출을 거부합니다.

### 키 종류와 기능

- **개인 키**는 한 사용자에게 속합니다. 권한은 키의 기능과 해당 사용자의 현재 권한이 겹치는 범위입니다. 사용자가 테넌트에 대한 접근 권한을 잃으면 키는 동작하지 않습니다.
- **서비스 키**는 테넌트에 속합니다. 키를 만든 사용자의 구성원 자격에 의존하지 않습니다. 테넌트의 Owner 또는 Admin만 서비스 키를 만들고 관리할 수 있습니다.
- 각 키는 `CAPABILITY_TRANSLATOR`와 `CAPABILITY_EXTRACTOR` 중 하나 또는 둘 모두를 가집니다. 키는 자신의 기능에 해당하는 도구만 호출할 수 있습니다.
- 사용자는 자신의 개인 키를 직접 관리합니다. 키 관리에는 Delino 로그인 토큰이 필요합니다. API 키로는 키를 만들거나 폐기할 수 없습니다.

### 키 관리

- `CreateKey`: `tenantId`, `kind`(`KEY_KIND_PERSONAL` 또는 `KEY_KIND_SERVICE`), `capabilities`, 선택 사항인 `expiresAt`을 보냅니다. `capabilities`에는 기능이 하나 이상 있어야 하며, `CAPABILITY_TRANSLATOR`와 `CAPABILITY_EXTRACTOR`는 각각 최대 한 번만 쓸 수 있습니다. 값을 반복하면 invalid-input 오류를 반환합니다.
- `ListKeys`: 관리할 수 있는 테넌트의 키를 `kind`로 필터링해 나열하며, `afterId`와 `pageSize`로 페이지를 나눕니다. `pageSize`는 생략하거나 0을 보내면 50이 됩니다. 최댓값은 100이며, 더 큰 값은 invalid-input 오류를 반환합니다. 다른 사용자의 개인 키는 절대 나열할 수 없습니다. `KEY_KIND_PERSONAL`은 본인의 개인 키를 반환합니다. `KEY_KIND_SERVICE`는 테넌트의 서비스 키를 반환하며, Owner 또는 Admin만 사용할 수 있습니다. `kind`를 생략하면 본인의 개인 키를 반환하고, Owner 또는 Admin이면 테넌트의 서비스 키도 함께 반환합니다. 응답에는 한 페이지가 담깁니다. `nextAfterId`가 비어 있지 않으면 키가 더 있습니다. 그 값을 다음 요청의 `afterId`로 보내고, `nextAfterId`가 빌 때까지 반복합니다. `nextAfterId`가 비어 있으면 전체 목록을 받은 것입니다.
- `RevokeKey`: `tenantId`와 `keyId`를 보냅니다. 폐기는 멱등입니다. 이미 폐기된 키에 대해 반복해도 성공하며 아무것도 바뀌지 않으므로, 응답이 유실되거나 시간 초과가 발생하면 같은 요청을 안전하게 다시 보낼 수 있습니다.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/CreateKey" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"tenantId":"<tenant-id>","kind":"KEY_KIND_SERVICE","capabilities":["CAPABILITY_TRANSLATOR"]}'
```

- **일회성 비밀 값.** `CreateKey` 응답에는 `secret`이 한 번만 포함됩니다. DeliTools는 검증용 해시만 저장하며 비밀 값을 다시 보여 줄 수 없습니다. 키를 잃어버렸다면 새 키를 만드세요. 비밀 값은 비밀 관리 도구에 저장하세요.
- **만료.** `expiresAt`은 선택 사항입니다. 보내는 경우 미래의 시각이어야 하며, 그렇지 않으면 `CreateKey`가 invalid-input 오류를 반환합니다. 만료 시각이 없는 키는 폐기할 때까지 유효합니다. 개인 키는 소유자가 테넌트 접근 권한을 유지해야 합니다. 서비스 키는 생성한 사용자에 의존하지 않으므로 그 사용자를 제거해도 중지되지 않습니다. 서비스 키를 폐기하거나 만료 시각을 지정하세요.
- **폐기.** 키를 폐기하거나 개인 키 또는 로그인 토큰의 사용자가 권한을 잃으면 새 요청은 거부되고, 실행 중인 호출은 더 이상 AI 재시도를 하지 않습니다. 이미 실행 중인 호출은 끝까지 진행될 수 있습니다. 그 사용량은 청구됩니다.

## 요금 청구

DeliTools는 사용량을 Delcore에 보고합니다. Delcore가 이를 USD로 요금을 산정하고 정산합니다.

- **기준.** 기준은 Vercel AI Gateway 모델 카탈로그에 있는 모델의 공개 토큰 가격에 호출이 실제로 측정한 토큰 수를 곱한 값입니다. Gateway 할인과 프로모션 크레딧은 기준을 낮추지 않습니다.
- **마크업.** 번역 비용은 기준에 1.15를 곱한 값입니다. 추출 비용은 기준에 1.30을 곱한 값입니다. 금액은 정확한 소수와 Delcore의 반올림 규칙을 사용합니다.
- **가격 변경.** DeliTools는 카탈로그 가격을 자동으로 갱신합니다. 새 가격은 적용되기 시작한 뒤 시작되는 요청에 모든 고객에게 동일하게 적용됩니다. 요청은 모든 재시도에 걸쳐 시작 시점의 가격을 유지합니다. 가격 갱신이 실패하면 DeliTools는 마지막으로 동기화한 가격을 기간 제한 없이 계속 사용하므로, 갱신이 성공할 때까지 요청이 오래된 가격으로 청구될 수 있습니다. DeliTools에 모델 가격이 없으면 새 AI 실행을 거부합니다.
- **실패도 청구됩니다.** 재시도, 실패한 호출, 시간 초과, 취소, 받지 못한 결과에 사용된 토큰도 마크업과 함께 계산됩니다.
- **알 수 없는 사용량.** DeliTools가 호출의 사용량을 즉시 읽지 못하면 요금 부과를 보류하고 나중에 Gateway와 대조합니다. 요금 부과를 위해 사용량을 추정하지는 않습니다. 레코드가 만료되는 30일 후에도 사용량을 알 수 없으면 해당 요금은 폐기됩니다.
- **사용량 확인 위치.** 응답에는 사용량이 포함되지 않습니다. 사용량과 요금은 테넌트의 Delino 요금 청구 인터페이스에서 확인하세요.
- **차단된 테넌트.** 요금 청구가 테넌트를 차단하면 새 실행은 billing-blocked 오류로 실패합니다. 이미 실행 중인 AI 시도는 중단되지 않지만, DeliTools는 재시도하기 전에 매번 권한을 다시 확인합니다. 그때까지 요금 청구가 테넌트를 차단했다면 더 이상 시도하지 않고 billing-blocked 오류를 반환합니다. DeliTools는 실행 후에 사용량을 측정하므로, 동시 호출은 차단이 적용되기 전에 잔액이나 한도를 초과할 수 있습니다.

## 멱등성

`idempotencyKey`는 선택 사항입니다. 보내는 경우 눈에 보이는 ASCII 문자 1~256자(공백과 제어 문자 제외)여야 하며, 그 외의 키는 invalid-input 오류를 반환합니다. 키가 없으면 모든 호출이 새 실행이며 새 요금이 발생할 수 있습니다. 네트워크 실패 후 재시도하는 경우도 마찬가지입니다.

키가 있으면 DeliTools는 **테넌트, 메서드, 키**로 요청을 추적합니다. 이는 같은 테넌트의 서로 다른 자격 증명에 걸쳐 적용됩니다. DeliTools는 요청 상태를 알려 주기 전에 현재 권한을 확인합니다.

| 상황                              | 결과                                                                               |
| --------------------------------- | ---------------------------------------------------------------------------------- |
| 새 키                             | 호출이 정상적으로 실행됩니다.                                                      |
| 같은 키와 같은 입력, 아직 실행 중 | 첫 호출의 요청 ID와 함께 processing 오류를 반환합니다. 기다리지 않습니다.          |
| 같은 키와 같은 입력, 완료됨       | already-processed 오류를 반환합니다. 결과는 다시 제공되지 않습니다.                |
| 같은 키와 다른 입력               | input-conflict 오류를 반환합니다.                                                  |
| 첫 호출이 알 수 없는 상태로 끝남  | unknown-outcome 오류를 반환합니다. DeliTools는 AI 호출을 스스로 반복하지 않습니다. |
| 30일 레코드가 만료된 뒤 같은 키   | 새 호출이며 정상적으로 요금이 부과됩니다.                                          |

DeliTools는 키, 입력의 해시, 상태를 보관합니다. 입력이나 결과는 보관하지 않습니다. 중복 제거는 첫 호출 후 30일 동안만 유지됩니다. 결과를 받으면 저장하세요. 같은 키로 호출을 반복해도 결과를 다시 반환할 수 없습니다.

## 오류

메서드가 반환하는 모든 오류에는 Connect 코드, 안정적인 사유, 요청 ID, 영어 메시지가 있습니다. 사유와 요청 ID는 `delitools.v1.ErrorDetail` 오류 상세에 들어 있습니다. 코드에서는 메시지가 아닌 사유를 사용하세요. 메서드가 실행되기 전에 Connect 전송 계층이 발생시키는 오류에도 `ErrorDetail`이 포함됩니다. 잘못된 형식의 요청 본문 또는 지원하지 않는 콘텐츠 타입의 사유는 `ERROR_REASON_INVALID_INPUT`, 너무 큰 메시지의 사유는 `ERROR_REASON_LIMIT`, 알 수 없는 메서드의 사유는 `ERROR_REASON_UNSPECIFIED`이며 Connect 코드는 `unimplemented`입니다. 이 오류의 메시지는 일반적인 문구이며 요청 내용을 반복하지 않습니다. 요청 ID는 모든 응답에 포함되는 응답 헤더 `X-Delitools-Request-Id`에서 계속 확인할 수 있습니다. `ErrorDetail`이 있을 때만 거기서 사유를 읽으세요.

| 사유                                 | Connect 코드          | 의미                                                                         |
| ------------------------------------ | --------------------- | ---------------------------------------------------------------------------- |
| `ERROR_REASON_INVALID_INPUT`         | `invalid_argument`    | 요청이 올바르지 않습니다. 예: 빈 배열 또는 동일한 언어.                      |
| `ERROR_REASON_INVALID_SCHEMA`        | `invalid_argument`    | 스키마가 지원하는 하위 집합을 벗어납니다.                                    |
| `ERROR_REASON_INVALID_MODEL`         | `invalid_argument`    | 모델이 허용 목록에 없습니다.                                                 |
| `ERROR_REASON_UNAUTHENTICATED`       | `unauthenticated`     | 자격 증명이 없거나, 유효하지 않거나, 만료되었거나, 폐기되었습니다.           |
| `ERROR_REASON_FORBIDDEN`             | `permission_denied`   | 자격 증명이나 사용자가 이 테넌트에서 이 도구를 사용할 수 없습니다.           |
| `ERROR_REASON_BILLING_BLOCKED`       | `failed_precondition` | 요금 청구가 해당 테넌트의 새 유료 실행을 허용하지 않습니다.                  |
| `ERROR_REASON_PRICE_UNAVAILABLE`     | `failed_precondition` | 모델의 공개 가격이 없으므로 DeliTools가 실행하지 않습니다.                   |
| `ERROR_REASON_LIMIT`                 | `resource_exhausted`  | 설정된 제한을 초과했습니다. 예: 입력 크기, 항목 수, 속도, 동시성.            |
| `ERROR_REASON_PROCESSING`            | `aborted`             | 같은 멱등성 키의 요청이 아직 실행 중입니다.                                  |
| `ERROR_REASON_ALREADY_PROCESSED`     | `already_exists`      | 같은 멱등성 키와 같은 입력의 요청이 이미 완료되었습니다.                     |
| `ERROR_REASON_INPUT_CONFLICT`        | `failed_precondition` | 멱등성 키를 다른 입력에 사용했습니다.                                        |
| `ERROR_REASON_UNKNOWN_OUTCOME`       | `unknown`             | 첫 실행의 결과를 알 수 없습니다.                                             |
| `ERROR_REASON_EXTRACTION_IMPOSSIBLE` | `failed_precondition` | 텍스트에 스키마가 요구하는 정보가 없습니다.                                  |
| `ERROR_REASON_INVALID_OUTPUT`        | `data_loss`           | 모델이 형식이 잘못된 출력 또는 스키마에 맞지 않는 출력을 반환했습니다.       |
| `ERROR_REASON_PROVIDER`              | `unavailable`         | 허용된 재시도 후에도 AI 제공자가 실패했습니다.                               |
| `ERROR_REASON_TIMEOUT`               | `deadline_exceeded`   | 실행 제한 시간이 지났습니다.                                                 |
| `ERROR_REASON_CANCELED`              | `canceled`            | 호출이 취소되었습니다.                                                       |
| `ERROR_REASON_INTERNAL`              | `internal`            | 내부 실패입니다. 예: 저장소 실패.                                            |
| `ERROR_REASON_INTERNAL`              | `unavailable`         | 테넌트의 인증 또는 요금 청구 확인을 완료하지 못해 새 유료 작업이 거부됩니다. |

처리 방법:

- `invalid_argument`, `permission_denied`, `unauthenticated`, `failed_precondition` 오류는 재시도하기 전에 요청을 고치세요. billing-blocked 오류는 요금 청구 차단을 해소하세요. 예외는 `ERROR_REASON_PRICE_UNAVAILABLE`입니다. 이는 배포 상태를 나타냅니다. DeliTools는 작업을 시작하거나 멱등성 키를 저장하기 전에 호출을 거부하며, 가격 카탈로그가 복구되면 변경하지 않은 같은 요청이 나중에 성공할 수 있습니다.
- 제공자 오류, 시간 초과, 내부 오류는 나중에 재시도할 수 있습니다. 제공자 실패와 시간 초과도 요금이 청구될 수 있습니다. 첫 호출이 DeliTools에 도달했는지 알 수 없을 때(예: 네트워크 실패 후) 두 번째 실행을 피하도록 멱등성 키를 사용하세요. 키를 사용한 호출이 제공자 오류 같은 오류로 끝나면 같은 키는 `ERROR_REASON_ALREADY_PROCESSED`(결과를 알 수 없으면 `ERROR_REASON_UNKNOWN_OUTCOME`)를 반환하며 새 실행을 시작하지 않습니다. 작업을 의도적으로 다시 실행하려면 새 키를 보내세요. 새 실행에는 다시 요금이 발생할 수 있습니다. `ERROR_REASON_INTERNAL`과 함께 오는 `unavailable` 오류는 DeliTools가 인증 또는 요금 청구 확인을 완료하지 못했다는 뜻입니다. `ERROR_REASON_INTERNAL`은 `internal`과 함께 올 수도 있으므로 코드와 사유를 모두 기준으로 분기하세요.
- unknown, processing, already-processed 오류에는 첫 호출의 요청 ID가 들어 있습니다. 결과가 계속 알 수 없는 상태라면 그 ID로 지원팀에 문의하세요.

## 데이터 보관

DeliTools는 입력 텍스트, 스키마, 결과를 저장하지 않습니다. 이들을 로그, 트레이스, 감사 이벤트에도 남기지 않습니다. 자격 증명과 비밀 값은 텔레메트리에서 제외됩니다.

DeliTools는 서비스를 운영하고 요금을 청구하는 데 필요한 데이터만 저장합니다. API 키의 검증용 해시, 요청이 속한 사용자와 테넌트, 멱등성을 위한 입력 해시, 요청 상태, 시도 식별자, 측정된 토큰 사용량, 가격 스냅샷, 안전한 감사 및 운영 레코드입니다.

| 데이터                                      | 보관 기간                     |
| ------------------------------------------- | ----------------------------- |
| 실행 요청 레코드(멱등성 키 유무와 관계없음) | 생성 후 30일                  |
| 멱등성 레코드                               | 최초 생성 후 30일             |
| 사용량 전달 및 대조 레코드                  | 최초 생성 후 30일             |
| 플랫폼 감사 레코드와 운영 이벤트 레코드     | 생성 후 30일                  |
| API 키                                      | 30일 보관으로 삭제되지 않음   |
| Delcore에서 수락된 회계 항목                | Delcore 자체 보관 규칙에 따름 |

30일 보관 기간은 API 키에 적용되지 않습니다. 보관 정리 작업은 API 키를 삭제하지 않습니다. 만료와 폐기는 키 인증만 막으므로 검증 해시와 귀속 정보는 계속 저장됩니다.

보관 정리 작업은 DeliTools가 자체 데이터베이스에 저장한 레코드만 삭제합니다. 운영자가 로그나 트레이스를 수집기로 내보내면, 수집기는 자체 보관 규칙에 따라 이를 보관합니다. DeliTools는 운영자에게 그곳에서도 같은 30일을 적용하도록 요구하지만, 서비스가 이를 강제할 수는 없습니다. 내보낸 텔레메트리에는 사용자의 입력, 스키마, 결과가 포함되지 않습니다.

30일은 레코드가 처음 생성될 때 시작됩니다. 재시도와 상태 변경은 이 기간을 연장하지 않습니다. DeliTools는 완료되지 않은 요청과 아직 전달되지 않은 사용량을 포함해 모든 상태의 레코드가 만료되면 삭제합니다. 다음 두 가지 위험을 감수해야 합니다.

- 멱등성 레코드가 만료된 뒤 같은 키로 호출을 반복하면 다시 실행되고 다시 요금이 부과됩니다.
- 레코드가 만료될 때 아직 대기 중인 사용량은 사라지며, DeliTools는 이를 청구하지 않습니다.

## 외부 제공자의 데이터 처리

DeliTools는 작업을 수행하기 위해 사용자의 텍스트와, 추출의 경우 스키마를 Vercel AI Gateway로 보내고, 거기서 요청을 실행하는 모델의 제공자에게 보냅니다. Gateway로 보내는 각 요청에는 제공자 사용자 참조로 무작위 시도 ID(UUID)도 포함되며, DeliTools는 이를 사용량 대조에 사용합니다. DeliTools는 테넌트, 사용자, 키 식별자를 보내지 않습니다. 이 서비스들은 각자의 정책에 따라 데이터를 처리합니다. DeliTools에 데이터를 보내기 전에 정책을 읽어 보세요.

- [Vercel AI Gateway zero data retention](https://vercel.com/docs/ai-gateway/security-and-compliance/zdr)
- [Vercel AI Gateway security and compliance](https://vercel.com/docs/ai-gateway/security-and-compliance): Vercel이 데이터를 처리하고 보호하는 방식에 대한 Vercel 자체 설명입니다.
- [Vercel AI Gateway models](https://vercel.com/ai-gateway/models): 각 모델 페이지에 제공자가 나열되어 있습니다.
- 사용하는 모델 제공자의 데이터 정책.

DeliTools는 Gateway의 데이터 무보관 옵션을 보내지 않으며, 보관 정책을 기준으로 제공자를 선택하지도 않습니다. 어떤 제공자에서도 데이터 무보관을 약속하지 않습니다. 민감한 데이터를 감지하지 않으며, 입력에서 제거하지도 않습니다. 이 제공자들과 공유할 수 없는 데이터는 제출하지 마세요.

## 제한

DeliTools API 서버의 운영자는 입력 크기, 번역 항목 수, 실행 제한 시간, 테넌트당 분당 요청 수, 테넌트당 동시 요청 수, AI 재시도 횟수, 번역 모델 허용 목록의 제한을 설정합니다. 이들은 배포 설정이며 변경될 수 있습니다. 고정된 제품 제한이 아니며, DeliTools는 처리량이나 가용성을 보장하지 않습니다. 제한을 초과한 요청은 `ERROR_REASON_LIMIT`을 반환합니다. 다만 실행 제한 시간을 넘긴 호출은 `ERROR_REASON_TIMEOUT`을 반환합니다. 올바르지 않은 모델이면 `ERROR_REASON_INVALID_MODEL`을 반환합니다. 현재 값은 사용 중인 배포의 운영자에게 문의하세요.
