# DeliTools API

DeliTools는 Connect RPC로 동기식 문자열 배열 번역과 스키마 기반 데이터 추출을 제공합니다. 각 호출은 완료된 결과와 요청 ID 또는 형식이 지정된 오류를 반환합니다. 작업 큐나 결과 조회 API는 없습니다. 서비스 기본 URL, 테넌트 ID, 사용 가능한 번역 모델과 설정된 제한은 운영자에게 문의하세요.

## 인증과 키

모든 요청에 `Authorization: Bearer <token>`을 보냅니다. 설정된 Delino 리소스 대상의 Logto 사용자 액세스 토큰이나 DeliTools API 키를 사용하세요. 유효한 인증 정보 외에도 현재 테넌트 권한과 차단되지 않은 결제 계정이 필요합니다. 이 검사를 수행할 수 없으면 새로운 유료 실행을 중단합니다.

사용자 토큰으로 `delitools.v1.DeliToolsService/CreateKey`를 호출합니다. `tenantId`, `kind`(`KEY_KIND_PERSONAL` 또는 `KEY_KIND_SERVICE`), `capabilities`(`CAPABILITY_TRANSLATOR`, `CAPABILITY_EXTRACTOR` 또는 둘 다)를 지정하세요. 선택 사항인 `expiresAt`은 RFC 3339 타임스탬프입니다. 응답은 비밀 키를 한 번만 반환합니다. 안전하게 보관하세요. DeliTools는 키를 복구할 수 없으며, 분실했다면 새 키를 생성해야 합니다.

사용자는 자신의 개인 키를 관리합니다. 개인 키에는 사용자의 현재 권한이 계속 적용됩니다. 테넌트 Owner/Admin은 서비스 키를 관리합니다. 서비스 키는 테넌트에 속하며 발급자의 멤버십 유지 여부에 의존하지 않습니다. `ListKeys`는 비밀 값을 제외한 메타데이터를 반환하며 `tenantId`, `kind`, `afterId`, `pageSize`(1–100, 기본값 50)를 받습니다. `nextAfterId`로 다음 페이지를 조회하세요. `RevokeKey`는 `tenantId`와 `keyId`를 받습니다.

만료일이 없는 키는 폐기될 때까지 유효하지만 현재 권한 검사를 계속 받습니다. 키 폐기나 사용자 권한 제거는 새로운 실행과 추가 AI 재시도를 거부합니다. 이미 진행 중인 제공자 호출은 완료될 수 있으며 요금이 부과됩니다. 테넌트 관리자는 플랫폼 운영자 권한을 얻지 않습니다.

## 배열 번역

다음 JSON 필드로 `delitools.v1.DeliToolsService/TranslateTextArray`를 호출합니다.

```json
{
  "tenantId": "YOUR_TENANT_ID",
  "outputLanguage": "ko",
  "model": "openai/gpt-6-luna",
  "texts": ["Hello", "Good morning"],
  "idempotencyKey": "translation-example-1"
}
```

BCP 47 언어 태그를 사용하세요. `outputLanguage`는 필수이고 `inputLanguage`는 선택 사항입니다. 입력 언어가 없으면 모델이 배열 전체의 입력 언어 하나를 감지하며, 언어가 섞인 입력도 번역을 시도합니다. 출력 언어와 같은 언어가 자동 감지되어도 모델 처리를 생략하지 않습니다. 입력과 출력 언어를 명시적으로 같게 지정하거나 빈 배열 또는 빈 문자열을 보내면 유효하지 않은 입력입니다.

운영자가 허용한 모델만 사용할 수 있습니다. 응답의 `requestId`와 `texts`는 요청 ID와 원래 순서·배열 길이를 유지한 번역 결과입니다. 항목 하나라도 실패하면 요청 전체가 실패합니다. 부분 번역이나 실패 시 원문으로 대체한 결과를 반환하지 않습니다.

## 구조화된 데이터 추출

`delitools.v1.DeliToolsService/ExtractDataFromText`를 호출합니다.

```json
{
  "tenantId": "YOUR_TENANT_ID",
  "text": "Ada is 37 years old.",
  "jsonSchema": "{\"type\":\"object\",\"properties\":{\"name\":{\"type\":\"string\"},\"age\":{\"type\":\"integer\"}},\"required\":[\"name\",\"age\"],\"additionalProperties\":false}",
  "idempotencyKey": "extraction-example-1"
}
```

추출은 항상 `openai/gpt-6-luna`를 사용하며 호출자가 다른 모델을 선택할 수 없습니다. 응답은 `requestId`와, 승인된 스키마에 맞는 JSON 문자열인 `jsonData`를 포함합니다.

지원하는 스키마 부분 집합은 객체 루트와 중첩된 object, array, string, number, integer, boolean, null 타입입니다. `properties`, `required`, `additionalProperties: false`, 배열의 `items`, 선택적인 설명과 스칼라 `enum` 값을 사용할 수 있습니다. 모든 객체 속성은 필수여야 합니다. 참조, 임의의 키워드와 선택적 속성은 AI 실행 전에 거부합니다. 호출자 스키마는 중첩 8단계를 지원합니다. 제공자의 결합된 스키마 제한은 속성 5,000개, 열거형 값 1,000개, 속성 이름과 열거형 값의 총 문자 수 120,000자입니다. DeliTools는 응답 래퍼에 속성 하나와 문자 다섯 개를 예약합니다. 값이 250개를 초과하는 열거형의 문자열 값은 총 15,000자 이하여야 합니다.

필수 정보를 원문에서 찾을 수 없으면 모델은 값을 지어내지 않고 추출 불가를 반환하도록 지시받습니다. 서버는 출력 스키마를 검사하고 잘못된 형식의 결과를 거부합니다. 이 검사와 자동화된 의미 검증 픽스처가 완벽한 의미 정확성을 보장하지는 않습니다. 중요한 판단에 활용하기 전에 추출된 사실을 검토하세요.

## 멱등성과 알 수 없는 실행 결과

`idempotencyKey`는 선택 사항이며 최대 200바이트입니다. 키가 없으면 호출마다 새로 실행하고 요금이 부과될 수 있습니다. 중복 방지는 테넌트, 메서드와 키를 기준으로 하며, 같은 테넌트에서 서로 다른 인증 정보로 보낸 요청도 포함합니다. 중복 상태를 공개하기 전에 현재 권한을 검사합니다.

일치하는 요청이 진행 중이면 기존 요청 ID와 함께 처리 중 오류를 즉시 반환합니다. 완료된 요청의 중복 호출은 이미 처리됨 오류를 반환하며 결과를 재전송하지 않습니다. 같은 키에 입력을 변경하면 입력 충돌 오류를 반환합니다. 결과가 나중에 필요하면 호출자 측에서 보관하세요.

중단으로 실행 결과를 알 수 없게 될 수 있습니다. 결과를 복구하려는 목적으로 새 키를 사용해 재시도하지 마세요. 추가 실행에 추가 요금이 발생할 수 있습니다. 요청 ID를 운영자에게 전달해 상태 확인과 Gateway 사용량 대조를 요청하세요. DeliTools는 원래 요청을 다시 실행하거나 수동으로 입력한 요금을 받지 않습니다.

중복 방지와 미전달 사용량 기록은 처리나 대조가 해결되지 않았더라도 각 기록의 최초 생성 후 30일이 지나면 만료됩니다. 재시도는 이 기간을 연장하지 않습니다. 기록 삭제 후에는 같은 키로 새 유료 실행을 시작할 수 있습니다. 해결되지 않은 사용량은 만료 시 영구적으로 유실될 수 있습니다.

## 요금과 제한

번역 요금은 해당 Gateway 공개 토큰 가격에 1.15를 곱한 금액이고, 추출에는 1.30을 적용합니다. 실제로 측정된 토큰 사용량으로 비용을 계산하며 Gateway 할인과 프로모션 크레딧은 이 기준을 낮추지 않습니다. Delcore가 정확한 십진수 USD 회계와 최종 반올림을 담당합니다. 사용량과 요금은 실행 응답이 아닌 별도 결제 인터페이스에서 확인합니다.

가격은 요청 시작 시 고정되며 재시도 중에도 유지됩니다. 새로 동기화된 가격은 이후 시작하는 요청부터 모든 고객에게 적용됩니다. 카탈로그 장애 중에도 기존에 알려진 가격은 기간 제한 없이 사용할 수 있으며, 가격이 없으면 실행을 차단합니다. 이미 발생한 사용량은 실패, 시간 초과, 취소 또는 결과 전달 실패 후에도 과금 대상이며, 재시도에서 측정한 사용량도 포함합니다. 알 수 없는 사용량은 추정하지 않고 대조합니다. 실행 후 과금으로 모든 동시 초과 지출을 방지할 수는 없습니다.

운영자는 입력 바이트 수, 번역 항목 수, 시간 제한, 테넌트 요청 빈도, 동시 실행 수와 일시적 오류의 최대 재시도 횟수를 설정합니다. 여기에는 공통으로 적용되는 제품 할당량을 정하지 않습니다. 서비스는 잘못된 출력을 재시도하거나 다른 모델로 전환하지 않습니다. 클라이언트 취소 시 제공자 실행 취소를 시도하고 추가 재시도를 중단하지만 백그라운드 사용량 회계는 계속됩니다.

## 오류

모든 애플리케이션 오류에는 영어 메시지와 `reason`, `requestId`가 있는 `delitools.v1.ErrorDetail`이 포함됩니다. 메시지를 파싱하지 말고 안정적인 사유 식별자를 사용하세요. 아래 사유 식별자에서는 `ERROR_REASON_` 접두사를 생략했습니다.

| 사유                                               | Connect 코드          | 조치                                                      |
| -------------------------------------------------- | --------------------- | --------------------------------------------------------- |
| `INVALID_INPUT`, `INVALID_SCHEMA`, `INVALID_MODEL` | `invalid_argument`    | 요청을 수정하세요.                                        |
| `UNAUTHENTICATED`                                  | `unauthenticated`     | 인증 정보를 교체하거나 갱신하세요.                        |
| `FORBIDDEN`                                        | `permission_denied`   | 현재 테넌트와 키 권한을 확인하세요.                       |
| `BILLING_BLOCKED`, `PRICE_UNAVAILABLE`             | `failed_precondition` | 결제 접근을 복구하거나 운영자에게 문의하세요.             |
| `LIMIT`                                            | `resource_exhausted`  | 입력 크기나 요청 부하를 줄이세요.                         |
| `PROCESSING`                                       | `aborted`             | 기존 요청이 아직 실행 중입니다.                           |
| `ALREADY_PROCESSED`                                | `already_exists`      | 보관한 결과를 사용하세요. 결과 재전송은 없습니다.         |
| `INPUT_CONFLICT`                                   | `failed_precondition` | 입력을 변경할 때 같은 키를 재사용하지 마세요.             |
| `UNKNOWN_OUTCOME`                                  | `unknown`             | 운영자에게 확인을 요청하고 자동으로 다시 실행하지 마세요. |
| `EXTRACTION_IMPOSSIBLE`                            | `failed_precondition` | 누락된 원문 정보를 제공하세요.                            |
| `INVALID_OUTPUT`                                   | `data_loss`           | 모델이 완전하고 유효한 결과를 생성하지 못했습니다.        |
| `PROVIDER`                                         | `unavailable`         | 의존 서비스가 실패했습니다. 요청 ID를 보관하세요.         |
| `TIMEOUT`                                          | `deadline_exceeded`   | 실행이 설정된 시간 제한을 초과했습니다.                   |
| `CANCELED`                                         | `canceled`            | 호출자가 실행을 취소했습니다.                             |
| `INTERNAL`                                         | `internal`            | 진단을 위해 요청 ID를 보관하세요.                         |

## 데이터 처리

DeliTools는 원문, 스키마나 결과를 저장하지 않으며 이 데이터와 인증 정보를 로그, 트레이스와 감사 기록에서 제외합니다. 입력 비교 해시, 요청 상태와 사용량 귀속 정보는 보존 기간 동안 유지합니다. 플랫폼 감사 기록과 일반 운영 로그도 30일 후 만료됩니다. 이 보존 정책은 API 키를 만료시키거나 이미 승인된 회계 기록을 삭제하지 않습니다.

텍스트와 스키마는 Vercel AI Gateway와 모델 제공자에게 전송됩니다. Gateway는 설정 가능한 보존 및 프롬프트 학습 제어를 제공하며, 이후 데이터 처리는 제공자와 계정 설정에 따라 다릅니다. 데이터를 제출하기 전에 [Vercel의 데이터 처리 설명](https://vercel.com/i/what-is-an-ai-gateway)과 [Gateway 개인정보 보호 제어](https://vercel.com/changelog/zero-data-retention-no-prompt-training-on-ai-gateway)를 검토하세요. OpenAI는 기본적으로 API 데이터를 학습에 사용하지 않는다고 명시하며, 남용 모니터링 보존과 예외는 [데이터 제어](https://developers.openai.com/api/docs/guides/your-data)에 따릅니다. DeliTools는 모든 제공자에 대한 무보존을 약속하거나 민감한 데이터를 대신 감지하지 않습니다.
