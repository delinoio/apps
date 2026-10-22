# DeliTools

DeliTools는 개발자를 위한 두 가지 유료 API를 제공합니다. 텍스트 목록 번역과 텍스트에서 JSON 값 추출입니다. 자체 서버 측 코드에서 Connect 클라이언트 또는 HTTP를 통한 일반 JSON으로 호출할 수 있습니다. Delcore가 API 키, 권한, 청구를 처리하므로 모든 작업에 동일한 Delino 계정과 테넌트를 사용할 수 있습니다.

## 개념

- **번역:** 텍스트 목록, 대상 언어, 모델을 전송합니다. 동일한 순서와 개수의 번역 결과를 받습니다.
- **추출:** 텍스트와 JSON Schema를 전송합니다. 해당 스키마를 따르는 JSON 문서 하나를 받습니다.
- **테넌트:** Delino 개인 공간 또는 조직입니다. Delcore는 사용 요금을 테넌트에 청구합니다.
- **인증 정보:** Delino 로그인 토큰, 개인 API 키 또는 테넌트 서비스 API 키입니다. [API 키](/ko/services/delitools/api-keys)를 참조하세요.
- **요청 ID:** 모든 응답에는 요청 ID가 포함됩니다. 지원을 요청할 때 이 ID를 포함하세요.

## 로그인 및 권한

각 요청에 `Authorization: Bearer <credential>` 헤더를 전송하세요.

| 인증 정보          | 형식                      | 테넌트                                                |
| ------------------ | ------------------------- | ----------------------------------------------------- |
| Delino 로그인 토큰 | Delino 액세스 토큰        | 요청에 `tenantId`를 설정하세요. 필수 항목입니다.      |
| 개인 API 키        | `dlk_`로 시작하는 비밀 키 | 해당 키의 테넌트입니다. `tenantId`는 선택 사항입니다. |
| 테넌트 서비스 키   | `dlk_`로 시작하는 비밀 키 | 해당 키의 테넌트입니다. `tenantId`는 선택 사항입니다. |

API 키와 함께 `tenantId`를 설정하는 경우 해당 키의 테넌트여야 하며, 그렇지 않으면 요청이 `ERROR_REASON_PERMISSION_DENIED`로 실패합니다. 모든 요청은 요청 시점에 Delcore의 허용을 받아야 합니다. 인증 정보에 `translate` 또는 `extract` 기능 권한이 있어야 하고, 테넌트가 활성 상태여야 하며, 테넌트가 일반적인 청구 검사를 통과해야 합니다. 이 API는 유료이며, 잔액 소진과 같은 청구 문제가 테넌트에 발생하면 Delcore가 API 사용을 차단합니다.

## 빠른 시작

1. **인증 정보를 준비하세요.** Delino 로그인 토큰을 사용하거나 [API 키](/ko/services/delitools/api-keys)에 설명된 대로 API 키를 생성하세요.
2. **테넌트 ID와 모델을 확인하세요.** 테넌트 ID는 Delino 계정에 포함되어 있습니다. 사용 가능한 번역 모델은 서비스 운영자가 설정합니다.
3. **API를 호출하세요.** 모든 프로시저는 `delitools.v1.DelitoolsService`에 있습니다. `DELITOOLS_URL`을 서비스 운영자가 제공하는 DeliTools API 서버의 기본 URL로 설정하세요.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Translate" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"texts":["Hello","Good morning"],"targetLanguage":"fr","model":"<model-id>"}'
```

이 예제에서는 테넌트 정보가 포함된 API 키를 사용합니다. Delino 로그인 토큰을 사용하는 경우 JSON 본문에 `"tenantId":"<tenant-id>"`를 추가하세요.

JSON의 필드 이름은 lowerCamelCase를 사용합니다. 모든 필드, 지원되는 스키마 하위 집합, 오류 사유는 [API 참조](/ko/services/delitools/api-reference)를 확인하세요.

서버 측 코드나 명령줄에서 DeliTools를 호출하세요. 프런트엔드 코드에 API 키를 넣지 마세요. DeliTools는 브라우저에서의 호출을 지원하지 않습니다.

## 사용자 콘텐츠와 AI 제공업체

DeliTools는 Vercel AI Gateway에서 요청을 실행합니다. **제출한 텍스트와 스키마는 Vercel AI Gateway 및 선택한 모델의 제공업체로 전송됩니다.** 콘텐츠 보관을 포함한 해당 업체의 콘텐츠 처리에는 DeliTools가 아닌 해당 업체의 자체 약관과 설정이 적용됩니다.

- DeliTools는 텍스트, 스키마 또는 결과를 저장하지 않으며 로그에도 기록하지 않습니다. 이는 DeliTools에만 해당하는 설명입니다. Vercel AI Gateway나 모델 제공업체가 무엇을 보관하는지에 대한 보장은 아닙니다.
- 해당 제공업체와 공유할 권한이 없는 콘텐츠를 제출하지 마세요. 민감한 데이터나 규제 대상 데이터를 제출하기 전에 해당 업체의 약관을 검토하세요.
- DeliTools는 테넌트에 요금을 청구하는 데 필요한 사용 기록만 보관하며, 생성된 지 30일이 지나면 삭제합니다.
- 모델 출력의 정확성은 보장되지 않습니다. 결과를 활용하기 전에 확인하세요.

## 추가 가이드

- [API 참조](/ko/services/delitools/api-reference): 필드, 언어 태그, 스키마 하위 집합, 오류 및 제한.
- [API 키](/ko/services/delitools/api-keys): 개인 키와 테넌트 키, 권한, 만료 및 폐기.
- [청구](/ko/services/delitools/billing): 토큰 사용량이 요금으로 산정되는 방식.
- [문제 해결](/ko/services/delitools/troubleshooting): 클라이언트 설정 및 오류 해결.
