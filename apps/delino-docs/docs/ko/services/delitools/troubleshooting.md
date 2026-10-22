# DeliTools 구성 및 문제 해결

이 페이지에서는 DeliTools용 클라이언트를 설정하고 오류를 해결하는 방법을 안내합니다. 필드 규칙은 [API 참조](/ko/services/delitools/api-reference)를 확인하세요.

## 클라이언트 구성

- **기본 URL.** 서비스 운영자가 제공한 DeliTools API 서버 URL을 사용하세요. 프로시저 경로는 `/delitools.v1.DelitoolsService/Translate` 및 `/delitools.v1.DelitoolsService/Extract`입니다.
- **프로토콜.** Connect 클라이언트를 사용하거나 `POST` 및 `Content-Type: application/json`으로 JSON을 전송하세요. JSON 필드 이름은 lowerCamelCase를 사용합니다.
- **인증 정보.** `Authorization: Bearer <credential>`을 전송하세요. Delino 로그인 토큰이나 `dlk_`로 시작하는 API 키를 사용하세요. 키는 서버 측 코드에 보관하세요. DeliTools는 브라우저에서의 호출을 지원하지 않습니다.
- **테넌트.** 로그인 토큰에는 `tenantId`가 필요합니다. API 키는 이미 해당 테넌트를 식별합니다. `tenantId`도 함께 전송하는 경우에는 일치해야 합니다.
- **모델.** 운영자가 허용한 모델 식별자를 사용하세요. 추출은 고정된 모델을 사용하며 `model` 필드가 없습니다.
- **시간 초과.** 가장 큰 요청에 적합한 클라이언트 시간 제한을 설정하세요. 제공자 호출에는 몇 초가 걸릴 수 있으며, DeliTools가 일시적인 실패를 재시도할 수 있으므로 전체 호출에는 더 오래 걸릴 수 있습니다. 운영자의 실행 시간 제한을 초과한 요청은 `ERROR_REASON_TIMEOUT`을 반환합니다. 자체 시간 제한으로 호출이 먼저 종료되면 DeliTools는 이를 취소로 처리합니다.
- **재시도.** `ERROR_REASON_RATE_LIMITED`, `ERROR_REASON_DEPENDENCY_UNAVAILABLE`, `ERROR_REASON_TIMEOUT`이 발생하면 지수 백오프와 짧은 무작위 지연을 적용해 재시도하세요. `ERROR_REASON_PROVIDER_FAILED`의 경우에는 Connect 코드가 `unavailable`일 때만 재시도하세요. 각 재시도는 과금될 수 있는 새로운 실행입니다. [과금](/ko/services/delitools/billing)을 확인하세요. 유효성 검사에 실패한 요청을 변경 없이 재시도하지 마세요.

## 오류 해결

| 사유                                  | 조치                                                                                                                                                                                                                                                                                             |
| ------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `ERROR_REASON_INVALID_INPUT`          | 요청을 수정하세요. 빈 문자열, 언어 태그, 테넌트 ID, 모델, 스키마가 유효한 JSON인지 여부를 확인하세요.                                                                                                                                                                                            |
| `ERROR_REASON_UNAUTHENTICATED`        | 토큰이나 키를 확인하세요. 잘못 입력되었거나, 만료되었거나, 폐기되었거나, 삭제된 테넌트에 속해 있을 수 있습니다. 비밀 키 값을 분실했다면 새 키를 생성하세요. 모든 유효한 키가 갑자기 실패한다면 서비스 운영자에게 알리세요. 서비스 ID가 잘못 구성되면 키가 유효하지 않은 것으로 보일 수 있습니다. |
| `ERROR_REASON_PERMISSION_DENIED`      | 키가 `translate` 또는 `extract` 권한을 부여하는지, 사용자가 여전히 테넌트 내에서 권한을 가지고 있는지, 전송한 `tenantId`가 키의 테넌트인지 확인하세요.                                                                                                                                           |
| `ERROR_REASON_BILLING_BLOCKED`        | Delcore에서 잔액이나 갱신 등의 결제 문제를 해결한 다음 재시도하세요.                                                                                                                                                                                                                             |
| `ERROR_REASON_LIMIT_EXCEEDED`         | 항목 수를 줄이거나, 텍스트를 짧게 하거나, 스키마를 작게 하여 전송하는 데이터를 줄이세요. 큰 작업은 여러 요청으로 나누세요.                                                                                                                                                                       |
| `ERROR_REASON_RATE_LIMITED`           | 전송 속도를 낮추고 동시 요청 수를 줄인 뒤 잠시 후 재시도하세요.                                                                                                                                                                                                                                  |
| `ERROR_REASON_MODEL_UNAVAILABLE`      | 오류가 `invalid_argument`이면 허용되지 않는 모델이므로 다른 모델을 선택하세요. `failed_precondition`이면 운영자에게 모델 가격을 등록해 달라고 요청하세요.                                                                                                                                        |
| `ERROR_REASON_UNSUPPORTED_SCHEMA`     | [지원되는 하위 집합](/ko/services/delitools/api-reference)에 포함되지 않는 키워드를 제거하세요. 루트는 객체 스키마여야 합니다.                                                                                                                                                                   |
| `ERROR_REASON_PROVIDER_FAILED`        | Connect 코드가 `unavailable`이면 기다린 후 재시도하세요. `internal`이면 제공자가 요청을 거부한 것입니다. 변경 없이 재시도하지 말고 요청을 확인한 뒤 요청 ID와 함께 운영자에게 문의하세요.                                                                                                        |
| `ERROR_REASON_INVALID_OUTPUT`         | 한 번 재시도하세요. 반복되면 스키마나 입력을 단순화하세요. 해당 시도에는 요금이 부과됩니다.                                                                                                                                                                                                      |
| `ERROR_REASON_EXTRACTION_FAILED`      | 텍스트에 필수 정보가 없습니다. 누락된 세부 정보를 제공하거나 스키마에서 해당 속성을 선택 사항으로 설정하세요. 해당 시도에는 요금이 부과됩니다.                                                                                                                                                   |
| `ERROR_REASON_TIMEOUT`                | 더 작은 요청을 전송하세요. 나중에 재시도하세요.                                                                                                                                                                                                                                                  |
| `ERROR_REASON_CANCELED`               | 클라이언트가 연결을 종료했습니다. 클라이언트 시간 초과나 종료 여부를 확인하세요.                                                                                                                                                                                                                 |
| `ERROR_REASON_DEPENDENCY_UNAVAILABLE` | 나중에 재시도하세요. DeliTools는 권한을 확인할 수 없을 때 유료 작업을 거부합니다.                                                                                                                                                                                                                |

## 일반적인 문제

- **키가 한 요청에서는 작동하지만 다음 요청에서는 실패합니다.** 만료되었거나, 폐기되었거나, 권한을 잃은 사용자에게 속한 키는 즉시 작동을 중단합니다. `ListApiKeys`로 확인하세요.
- **사유 없이 `resource_exhausted` 오류만 표시됩니다.** 요청 본문이 운영자의 전체 크기 제한보다 큽니다. 전송하는 데이터를 줄이세요.
- **결과가 잘못된 것 같습니다.** 모델은 실수할 수 있으며, 스키마를 따르는 결과라고 해서 반드시 올바른 것은 아닙니다. 스키마의 `description` 필드에 있는 지침을 더 명확하게 작성하고 중요한 값을 확인하세요.
- **예상치 못한 요금이 발생했습니다.** 사용량이 확인된 실패한 시도와 반복 요청에는 요금이 부과됩니다. [과금](/ko/services/delitools/billing)을 확인하세요.
- **사용량이 아직 표시되지 않습니다.** 사용량은 호출 직후 잠시 후에 Delcore에 전달됩니다. 나중에 다시 확인하세요.

## 지원 요청

응답 본문이나 `X-Delitools-Request-Id` 헤더의 요청 ID, 프로시저 이름, 시간, 오류 사유를 포함하세요. API 키, 로그인 토큰 또는 제출한 텍스트는 보내지 마세요.
