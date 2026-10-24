# DeliTools API 참조

이 페이지에서는 DeliTools의 두 메서드를 설명합니다. 둘 다 `delitools.v1.DelitoolsService`의 Connect 프로시저이며 HTTP `POST`로 JSON을 받습니다. JSON의 필드 이름은 lowerCamelCase입니다. `Content-Type: application/json`과 `Authorization: Bearer <credential>`을 보내세요. 인증 정보와 테넌트에 대해서는 [DeliTools](/ko/services/delitools/)를 참조하세요.

메서드 핸들러가 생성하는 모든 응답에는 본문과 `X-Delitools-Request-Id` 헤더에 요청 ID가 포함됩니다. 전송 크기 제한으로 거부된 요청(오류 표 참조)에는 요청 ID가 없습니다. 응답에는 토큰 사용량이나 요금이 포함되지 않습니다. 이 정보는 Delcore에서 확인하세요. [결제](/ko/services/delitools/billing)를 참조하세요.

각 호출은 새로운 실행입니다. DeliTools에는 멱등성 키가 없으며 결과를 저장하지 않습니다. 요청을 반복하면 다시 실행되며 요금이 다시 부과될 수 있습니다.

## 번역

`POST /delitools.v1.DelitoolsService/Translate`

| 필드             | 유형        | 규칙                                                                                                                 |
| ---------------- | ----------- | -------------------------------------------------------------------------------------------------------------------- |
| `tenantId`       | 문자열      | Delino 로그인 토큰을 사용하는 경우 필수입니다. API 키를 사용하는 경우 비워 두거나 해당 키의 테넌트를 지정합니다.     |
| `texts`          | 문자열 배열 | 필수입니다. 항목이 하나 이상이어야 합니다. 빈 문자열은 허용되지 않습니다. 항목 수와 전체 크기는 운영자가 제한합니다. |
| `targetLanguage` | 문자열      | 필수입니다. `fr` 또는 `pt-BR`과 같은 BCP 47 언어 태그입니다.                                                         |
| `sourceLanguage` | 문자열      | 선택 사항입니다. BCP 47 언어 태그입니다. 생략하면 모델이 원문 언어를 감지합니다.                                     |
| `model`          | 문자열      | 필수입니다. 운영자가 허용하는 모델 식별자입니다.                                                                     |

응답에는 `requestId`와 `translations`가 포함됩니다. `translations`의 항목 수와 순서는 `texts`와 동일합니다.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Translate" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"texts":["Hello","Good morning"],"sourceLanguage":"en","targetLanguage":"fr","model":"<model-id>"}'
```

```json
{
  "requestId": "<request-id>",
  "translations": ["Bonjour", "Bonjour, bonne journée"]
}
```

- **언어 태그.** DeliTools는 대소문자를 구분하지 않고 태그가 BCP 47에 따라 올바른 형식인지 확인하며, 최대 64자이고 기본 언어가 두 글자 또는 세 글자인지 확인합니다. 사적 사용 하위 태그로만 구성된 태그와 이전 규격 호환 태그(grandfathered tag)는 거부됩니다. DeliTools는 모델이 해당 언어를 지원하는지는 확인하지 않습니다.
- **전부 성공하거나 전부 실패합니다.** 결과는 원자적으로 처리됩니다. 항목 하나라도 실패하면 전체 요청이 실패하며 부분 번역은 제공되지 않습니다.
- **특수 처리는 없습니다.** 원문 언어가 생략된 입력, 여러 언어가 혼합된 입력, 이미 대상 언어로 작성된 텍스트도 모두 모델로 전달됩니다. DeliTools는 이를 거부하지 않으며 원문 텍스트를 대신 복사해 주지도 않습니다.
- **모델.** 운영자가 허용하고 Delcore에 가격이 등록된 모델만 사용할 수 있습니다. 다른 모델로 대체되지 않습니다. 사용 가능한 모델 식별자는 서비스 운영자에게 문의하세요.

## 추출

`POST /delitools.v1.DelitoolsService/Extract`

| 필드         | 유형   | 규칙                                                                                                             |
| ------------ | ------ | ---------------------------------------------------------------------------------------------------------------- |
| `tenantId`   | 문자열 | Delino 로그인 토큰을 사용하는 경우 필수입니다. API 키를 사용하는 경우 비워 두거나 해당 키의 테넌트를 지정합니다. |
| `text`       | 문자열 | 필수이며 비어 있으면 안 됩니다. 크기는 운영자가 제한합니다.                                                      |
| `jsonSchema` | 문자열 | 필수입니다. 아래에서 지원하는 하위 집합만 사용하는 JSON Schema 문서를 JSON 문자열로 지정합니다.                  |

모델은 `openai/gpt-6-luna`로 고정되어 있습니다. 직접 선택할 수 없습니다. 응답에는 `requestId`와 스키마를 준수하는 JSON 문자열인 `json`이 포함됩니다.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Extract" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"text":"Please send three boxes of green tea.","jsonSchema":"{\"type\":\"object\",\"properties\":{\"item\":{\"type\":\"string\"},\"quantity\":{\"type\":\"integer\",\"minimum\":1}},\"required\":[\"item\",\"quantity\"],\"additionalProperties\":false}"}'
```

```json
{
  "requestId": "<request-id>",
  "json": "{\"item\":\"green tea\",\"quantity\":3}"
}
```

- **근거 기반 추출은 지시 사항이지 검증 항목이 아닙니다.** DeliTools는 모델에 사용자의 텍스트에서만 값을 가져오도록 지시합니다. 모델이 필수 정보가 없다고 보고하면 요청은 `ERROR_REASON_EXTRACTION_FAILED`로 실패합니다. DeliTools는 반환된 값이 사용자의 텍스트에서 나온 것인지 확인할 수 없으므로, 모델이 스키마를 준수하는 임의로 지어낸 값을 반환할 수도 있습니다.
- **출력 검증.** DeliTools는 결과가 사용자의 스키마를 준수하는지 검증합니다. 형식이 잘못되었거나 스키마를 준수하지 않는 출력은 `ERROR_REASON_INVALID_OUTPUT`으로 실패합니다. DeliTools는 잘못된 출력을 수정하기 위해 재시도하지 않습니다.
- **정확도.** 스키마를 준수하는 결과라도 틀릴 수 있습니다. 중요한 결과는 확인하세요.
- **지원되지 않는 스키마는 먼저 거부됩니다.** DeliTools는 모델을 실행하기 전에 지원되는 하위 집합을 벗어나는 스키마를 거부하므로, 이러한 요청에는 요금이 부과되지 않습니다.

### 지원되는 JSON Schema 하위 집합

스키마는 JSON Schema 2020-12로 해석됩니다. JSON 객체여야 합니다. `$schema`는 선택 사항이며 루트에서만 허용됩니다. 문자열이어야 하며, 그렇지 않으면 지원되지 않는 스키마로 처리됩니다. DeliTools는 이 문자열을 사용해 드래프트를 선택하지 않습니다. 루트, 각 속성 스키마, `items` 스키마를 포함한 모든 스키마에는 `type`, `enum` 또는 `const`가 필요합니다. 스키마의 깊이는 최대 8단계이고 스키마 노드는 최대 256개이며, 크기는 운영자가 제한합니다.

| 키워드                   | 규칙                                                                                                                                                                  |
| ------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `type`                   | `object`, `array`, `string`, `number`, `integer`, `boolean`, `null` 또는 이러한 유형의 배열입니다. 예를 들어 null을 허용하는 값에는 `["string","null"]`을 사용합니다. |
| `properties`, `required` | `properties`는 이름을 스키마에 매핑합니다. 모든 `required` 이름은 `properties`에 있어야 합니다. 객체당 속성은 최대 64개이며 `required` 이름은 중복될 수 없습니다.     |
| `additionalProperties`   | `true` 또는 `false`만 허용됩니다(스키마는 허용되지 않음).                                                                                                             |
| `items`                  | 모든 항목에 적용되는 하나의 스키마입니다.                                                                                                                             |
| `enum`, `const`          | `enum`에는 서로 다른 값이 1~64개 있어야 합니다. `const`는 단일 값입니다.                                                                                              |
| `minLength`, `maxLength` | 문자열에 적용되는 0~1,000,000 범위의 정수입니다. 최솟값은 최댓값을 초과할 수 없습니다.                                                                                |
| `minItems`, `maxItems`   | 배열에 적용되는 0~1,000,000 범위의 정수입니다. 최솟값은 최댓값을 초과할 수 없습니다.                                                                                  |
| `minimum`, `maximum`     | `number`와 `integer`에 적용되는 유한한 수입니다. 최솟값은 최댓값을 초과할 수 없습니다.                                                                                |
| `title`, `description`   | 모델을 위한 참고 사항입니다. 검증에는 영향을 주지 않습니다.                                                                                                           |

다음은 지원되지 않으며 `ERROR_REASON_UNSUPPORTED_SCHEMA`를 반환합니다. `$ref`, `$defs`, `definitions`, `$id`, `$comment`, `oneOf`, `anyOf`, `allOf`, `not`, `if`/`then`/`else`, `patternProperties`, `propertyNames`, `pattern`, `format`, `uniqueItems`, `multipleOf`, `exclusiveMinimum`, `exclusiveMaximum`, `contains`, `prefixItems`, `default`, `unevaluated` 키워드, 불리언 스키마, 알 수 없는 키워드, 크기가 너무 크거나 중첩 깊이가 너무 깊은 스키마. DeliTools는 참조를 따라가지 않으므로 스키마를 통해 어떤 항목도 가져오게 할 수 없습니다.

나열한 속성만 허용하려면 객체에 `"additionalProperties": false`를 설정하세요.

## 오류

오류는 표준 Connect 코드를 사용합니다. 오류 세부 정보에는 고정된 사유 코드와 요청 ID가 포함됩니다. 메시지는 영어로 제공되며 사용자의 콘텐츠는 절대 포함하지 않습니다.

| 사유                                  | Connect 코드                                  | 의미                                                                                                                                                                                                                                      |
| ------------------------------------- | --------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ERROR_REASON_INVALID_INPUT`          | `invalid_argument`                            | 필드가 유효하지 않습니다. 빈 배열 또는 문자열, 잘못된 언어 태그, 누락되거나 형식이 잘못된 테넌트, 누락된 모델, 유효한 JSON이 아닌 `jsonSchema`가 해당합니다. 운영자가 허용하지 않는 모델은 `ERROR_REASON_MODEL_UNAVAILABLE`로 처리됩니다. |
| `ERROR_REASON_UNAUTHENTICATED`        | `unauthenticated`                             | 인증 정보가 누락되었거나, 형식이 잘못되었거나, 만료되었거나, 폐기되었거나, 알 수 없는 정보입니다.                                                                                                                                         |
| `ERROR_REASON_PERMISSION_DENIED`      | `permission_denied`                           | 인증 정보에 필요한 기능 권한, 키에 부여된 권한 또는 사용자의 테넌트 권한이 없거나, 전송한 `tenantId`가 해당 키의 테넌트가 아닙니다.                                                                                                       |
| `ERROR_REASON_BILLING_BLOCKED`        | `failed_precondition`                         | 잔액, 갱신, 분쟁, 운영자의 차단 또는 테넌트 삭제 등의 이유로 Delcore가 유료 사용을 차단했습니다.                                                                                                                                          |
| `ERROR_REASON_LIMIT_EXCEEDED`         | `resource_exhausted`                          | 입력, 항목 수 또는 스키마가 운영자의 제한을 초과합니다.                                                                                                                                                                                   |
| `ERROR_REASON_RATE_LIMITED`           | `resource_exhausted`                          | 테넌트가 요청 빈도 또는 동시 실행 제한에 도달했습니다. 나중에 다시 시도하세요. 요청 빈도 오류에는 `Retry-After` 헤더가 포함됩니다.                                                                                                        |
| `ERROR_REASON_MODEL_UNAVAILABLE`      | `invalid_argument` 또는 `failed_precondition` | 모델이 허용되지 않거나(`invalid_argument`), 등록된 입력 및 출력 가격이 없습니다(`failed_precondition`). 다른 모델로 대체되지 않습니다.                                                                                                    |
| `ERROR_REASON_UNSUPPORTED_SCHEMA`     | `invalid_argument`                            | 스키마가 지원되는 하위 집합에 포함되지 않는 키워드를 사용합니다.                                                                                                                                                                          |
| `ERROR_REASON_PROVIDER_FAILED`        | `unavailable` 또는 `internal`                 | AI 제공자가 허용된 재시도 후에도 계속 실패했거나(`unavailable`), 요청을 거부했습니다(`internal`).                                                                                                                                         |
| `ERROR_REASON_INVALID_OUTPUT`         | `internal`                                    | 모델 출력의 형식이 잘못되었거나, 중간에 잘렸거나, 규약을 준수하지 않습니다.                                                                                                                                                               |
| `ERROR_REASON_EXTRACTION_FAILED`      | `failed_precondition`                         | 텍스트에 필수 정보가 없습니다.                                                                                                                                                                                                            |
| `ERROR_REASON_TIMEOUT`                | `deadline_exceeded`                           | 운영자가 설정한 실행 제한 시간이 초과되었습니다. 이 제한 시간에는 검사와 모든 시도가 포함됩니다.                                                                                                                                          |
| `ERROR_REASON_CANCELED`               | `canceled`                                    | 요청이 취소되었거나 연결이 닫혔습니다.                                                                                                                                                                                                    |
| `ERROR_REASON_DEPENDENCY_UNAVAILABLE` | `unavailable`                                 | 권한, 가격 또는 결제 확인을 완료할 수 없거나 DeliTools가 호출의 사용량을 기록할 수 없습니다. 유료 사용은 안전을 위해 차단되며 결과가 제공되지 않습니다.                                                                                   |

운영자의 전체 요청 크기 제한을 초과하는 요청은 이러한 세부 정보 없이 일반 `resource_exhausted` 오류를 받을 수 있습니다. 대처 방법은 [문제 해결](/ko/services/delitools/troubleshooting)을 참조하세요.

잘못된 유형의 값, 잘린 본문 또는 구문 오류 등으로 인해 디코딩할 수 없는 JSON 요청 본문에는 HTTP 400이 반환되며, Connect 코드 `invalid_argument`, 사유 `ERROR_REASON_INVALID_INPUT`, 요청 ID, 그리고 정확히 `request body is invalid`라는 메시지가 포함됩니다. DeliTools는 자격 증명을 확인하기 전에 본문을 검사합니다. 메시지는 사용자 콘텐츠의 어떤 부분도 반복하지 않습니다. JSON 인코딩 또는 필드 유형을 수정하고 요청을 다시 전송하세요.

## 제한 및 재시도

운영자가 다음 제한을 설정하며, 제한은 변경될 수 있습니다. 입력 크기, 번역 항목 수, 스키마 크기 및 깊이, 실행 제한 시간, 테넌트별 요청 빈도 및 버스트, 테넌트별 동시 실행 수, 일시적 오류에 대한 최대 재시도 횟수. 적용되는 값은 서비스 운영자에게 문의하세요. 제한은 보호 장치이며 처리량을 보장하지 않습니다.

- DeliTools는 네트워크 오류, 서버 오류, 요청 빈도 제한 응답과 같은 일시적인 AI 제공자 오류에 대해서만 설정된 횟수까지 재시도합니다. 잘못된 출력을 수정하기 위한 재시도는 하지 않습니다.
- 각 재시도 전에 DeliTools는 권한을 다시 확인합니다. 키가 폐기되거나 만료된 경우, 권한을 잃은 경우 또는 결제가 차단된 경우 DeliTools는 중단하고 해당 오류를 반환합니다. 이미 실행 중인 제공자 호출은 완료될 수 있습니다.
- 요청을 취소하거나 연결을 닫으면 DeliTools는 제공자 호출을 취소하고 재시도를 중단합니다.
- 클라이언트가 호출을 닫거나 자체 제한 시간이 초과되어 호출을 종료하면 DeliTools는 이를 취소로 처리합니다.
- 운영자가 설정한 실행 제한 시간에는 권한 확인, 검증 및 모든 시도가 포함됩니다. 제한 시간이 초과되면 DeliTools는 제공자 호출을 중단하고 `ERROR_REASON_TIMEOUT`을 반환합니다.
