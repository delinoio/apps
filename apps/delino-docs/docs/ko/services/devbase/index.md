# DevBase

DevBase는 애플리케이션의 최종 사용자를 모아 두는 무료 디렉터리입니다. Connect API로 직접 만든 사용자를 등록하고, 조회하고, 변경합니다. 사용자는 Delino 계정이 필요 없으며, 로그인은 애플리케이션이 계속 맡습니다. DevBase는 각 사용자에게 다른 서비스에 전달할 수 있는 고정 ID를 부여합니다.

## 개념

- **테넌트:** Delino 개인 공간 또는 조직입니다. Delcore가 테넌트와 구성원을 관리합니다.
- **프로젝트:** 테넌트 안에서 최종 사용자를 모아 두는 이름 있는 디렉터리입니다. 같은 테넌트의 프로젝트끼리도 사용자를 공유하지 않습니다.
- **최종 사용자:** 애플리케이션의 사람 또는 계정입니다. 직접 정하는 외부 ID, DevBase가 만드는 내부 ID, 선택 사항인 이름과 이메일, JSON 객체 메타데이터를 가집니다.
- **프로젝트 API 키:** 애플리케이션이 한 프로젝트의 최종 사용자를 읽거나 변경할 때 쓰는 비밀 값입니다.

## 로그인과 권한

각 요청에 `Authorization: Bearer <token>` 헤더를 보냅니다.

- **개발자**는 Delino 로그인 토큰을 사용합니다. 현재 테넌트의 Owner 또는 Admin만 프로젝트와 API 키를 관리할 수 있습니다. 이들은 로그인 토큰으로 최종 사용자도 읽고 변경할 수 있습니다.
- **애플리케이션**은 프로젝트 API 키를 사용합니다. 키는 `dbk_`로 시작합니다. `READ_ONLY` 키는 사용자를 조회하고 목록을 봅니다. `CRUD` 키는 사용자를 만들고, 수정하고, 삭제할 수도 있습니다. 키는 프로젝트나 키를 관리하지 못하며, 자신의 프로젝트에서만 동작합니다.

최종 사용자는 DevBase에 로그인하지 않습니다. 이들의 레코드로는 Delino에 접근할 수 없습니다.

## 빠른 시작

1. **프로젝트를 만듭니다.** 테넌트 ID와 이름으로 `CreateProject`를 호출합니다. 이름은 테넌트 안에서 대소문자를 구분하지 않고 고유해야 합니다.
2. **키를 발급합니다.** 프로젝트 ID와 권한으로 `IssueApiKey`를 호출합니다. 응답에는 비밀 값이 한 번만 포함됩니다. 비밀 관리 도구에 저장하세요.
3. **사용자를 등록합니다.** 키로 `CreateEndUser`를 호출합니다. 반환된 `id`를 직접 관리하는 사용자 레코드 옆에 저장합니다.
4. **사용자를 조회합니다.** 내부 ID로 `GetEndUser`를 쓰거나 외부 ID로 `LookupEndUser`를 씁니다.
5. **사용자를 변경하거나 삭제합니다.** 현재 `version`으로 `UpdateEndUser`와 `DeleteEndUser`를 씁니다.

모든 프로시저는 `devbase.v1.DevBaseService`에 있습니다. 어떤 Connect 클라이언트든 쓸 수 있으며, HTTP로 일반 JSON을 보내도 됩니다. `DEVBASE_URL`을 DevBase API 서버의 기본 URL로 설정하세요. 로컬 서버라면 `http://127.0.0.1:46317`입니다.

```sh
# Create a project with your Delino login token.
curl --request POST "$DEVBASE_URL/devbase.v1.DevBaseService/CreateProject" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"idempotencyKey":"3f1f6d1e-5a0a-4d7b-9a53-0c1d9f7a7d01","tenantId":"<tenant-id>","name":"My app"}'

# Issue a CRUD key for the project. The secret appears only in this response.
curl --request POST "$DEVBASE_URL/devbase.v1.DevBaseService/IssueApiKey" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"idempotencyKey":"8c2b7a60-91a0-4a53-a9c3-6f7f0b2f1e02","projectId":"<project-id>","permission":"API_KEY_PERMISSION_CRUD"}'

# Register an end user with the project key.
curl --request POST "$DEVBASE_URL/devbase.v1.DevBaseService/CreateEndUser" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DEVBASE_KEY" \
  --data '{"idempotencyKey":"d5f0c3a2-2b1e-4f59-8f0a-5b7c9e4d3a03","projectId":"<project-id>","externalId":"user-1001","email":"ada@example.com","metadata":{"plan":"pro"}}'
```

JSON에서 필드 이름은 lowerCamelCase를 쓰고, 열거형 값은 전체 이름을 쓰며, `version` 같은 64비트 숫자는 문자열입니다.

## 프로젝트

- 프로젝트 이름은 1~256바이트이며 앞뒤에 공백이 없어야 합니다.
- `UpdateProject`는 이름을 변경합니다. `DeleteProject`는 프로젝트와 모든 사용자, 모든 키를 한 번에 삭제합니다.
- 한 테넌트는 최대 100개의 프로젝트를 가질 수 있습니다.

## 프로젝트 API 키

- 프로젝트는 최대 20개의 활성 키를 가질 수 있습니다. 키는 폐기되거나 만료될 때까지 활성 상태입니다.
- 키가 만료되어야 한다면 발급할 때 `expire_time`을 설정합니다. 설정하지 않으면 키는 자동으로 만료되지 않습니다.
- DevBase는 비밀 값의 해시만 저장합니다. 비밀 값을 잃어버렸다면 해당 키를 폐기하고 새 키를 발급하세요. 키를 교체하려면 새 키를 먼저 발급한 뒤 이전 키를 폐기합니다.
- `IssueApiKey` 응답을 받지 못했다면 같은 멱등성 키로 요청을 반복하세요. 반복한 응답에는 키 ID와 권한이 있지만 비밀 값은 없습니다.
- `RevokeApiKey`는 즉시 적용됩니다. 폐기되었거나 만료된 키는 `unauthenticated`를 반환합니다.

## 최종 사용자

| 필드          | 규칙                                                                        |
| ------------- | --------------------------------------------------------------------------- |
| `external_id` | 필수. 1~256바이트. 대소문자를 구분. 프로젝트 안에서 고유. 앞뒤에 공백 없음. |
| `name`        | 선택. 최대 256바이트.                                                       |
| `email`       | 선택. 최대 254바이트의 일반 주소. 여러 사용자가 같은 이메일을 가질 수 있음. |
| `metadata`    | 최대 16 KiB의 JSON 객체. 전체 요청은 최대 64 KiB.                           |

- 같은 외부 ID가 서로 다른 프로젝트에 있을 수 있습니다. 한 프로젝트는 최대 1,000,000명의 사용자를 가질 수 있습니다.
- 생성은 명시적입니다. DevBase에는 upsert가 없고, 이메일·이름·메타데이터로 검색하는 기능도 없습니다.
- `UpdateEndUser`는 `external_id`, `name`, `email`, `metadata`를 가진 `update_mask`를 사용합니다. 마스크에 포함된 `name` 또는 `email`이 빈 값이면 지워집니다. 마스크에 포함된 `metadata`는 객체 전체를 교체하며, 빈 객체는 메타데이터를 지웁니다. 외부 ID는 지울 수 없지만 바꿀 수 있고, 내부 ID는 그대로 유지됩니다.
- `DeleteEndUser`는 사용자를 즉시 영구적으로 제거합니다. 외부 ID를 바로 다시 사용할 수 있습니다. 그 ID로 만든 다음 사용자는 새 내부 ID를 받으며 삭제된 사용자와 연결되지 않습니다.
- DevBase는 삭제 사실을 다른 서비스에 알리지 않습니다. 하위 데이터는 애플리케이션에서 직접 삭제하세요.

## 목록

`ListProjects`, `ListApiKeys`, `ListEndUsers`는 항목을 생성 순서로 반환합니다. 기본 페이지 크기는 50이고 최대는 100입니다. `next_page_token`이 빌 때까지 호출을 반복하세요. 토큰은 불투명한 값으로 취급합니다. 목록에는 전체 개수가 없습니다. 페이지를 읽는 동안 항목을 만들거나 삭제하면 페이지가 해당 항목을 놓치거나 건너뛸 수 있습니다. 읽는 내내 존재하는 항목은 한 번만 나타납니다.

## 재시도와 충돌

- 모든 변경에는 `A-Z`, `a-z`, `0-9`, `.`, `_`, `:`, `-` 문자로 이루어진 8~128자의 `idempotency_key`가 필요합니다. UUID가 적합합니다.
- 24시간 안에 같은 키와 같은 내용으로 요청을 반복하면 첫 결과를 받으며 DevBase는 다시 아무것도 변경하지 않습니다. 응답을 잃어버린 경우에 활용하세요. 24시간이 지나면 같은 키도 새 요청입니다.
- 같은 키에 다른 내용을 보내면 `already_exists`를 반환합니다. 호출자마다 키가 따로 관리되므로 다른 호출자는 내 결과를 보거나 가져갈 수 없습니다.
- 반복한 요청은 현재 권한으로 다시 검사합니다. 첫 호출과 반복 사이에 접근 권한이 사라질 수 있습니다.
- 사용자를 삭제한 뒤 그 사용자의 생성 또는 수정 요청을 반복하면 `not_found`를 반환하며 DevBase는 사용자를 다시 만들지 않습니다. 삭제 요청을 반복하면 성공합니다. 프로젝트를 삭제하면 그 프로젝트의 키는 동작하지 않습니다.
- `UpdateProject`, `UpdateEndUser`, `DeleteProject`, `DeleteEndUser`에는 현재 `version`이 필요합니다. 다른 사람이 먼저 항목을 변경했다면 `aborted`를 받습니다. 항목을 다시 읽고 새 버전으로 재시도하세요.

## 제한

| 제한                   | 기본값                     |
| ---------------------- | -------------------------- |
| 테넌트당 프로젝트      | 100                        |
| 프로젝트당 사용자      | 1,000,000                  |
| 프로젝트당 활성 키     | 20                         |
| 프로젝트당 사용자 요청 | 초당 1,000회, 버스트 2,000 |
| 테넌트당 관리 요청     | 초당 50회, 버스트 100      |

사용자 요청은 한 프로젝트의 모든 키와 로그인 토큰을 합산합니다. 관리 요청은 테넌트 단위로 합산합니다. 새 키를 만들어도 처리 용량은 늘지 않습니다. 제한을 초과하면 `resource_exhausted`를 받습니다. 속도 제한이면 오류와 `Retry-After` 헤더가 기다릴 시간을 알려 줍니다. 운영자가 이 제한을 변경할 수 있습니다. 제한은 서비스를 보호하기 위한 것이며 처리 속도를 보장하지 않습니다.

## 오류

| 코드                 | 의미                                                                             |
| -------------------- | -------------------------------------------------------------------------------- |
| `unauthenticated`    | 토큰이나 키가 없거나, 유효하지 않거나, 만료되었거나, 폐기되었습니다.             |
| `permission_denied`  | 역할이나 키 권한이 없거나 테넌트 상태가 허용하지 않습니다.                       |
| `not_found`          | 항목이 없거나, 다른 프로젝트에 속하거나, 삭제되었습니다.                         |
| `invalid_argument`   | 입력 값이 올바르지 않습니다.                                                     |
| `already_exists`     | 이름이나 외부 ID가 이미 있거나, 멱등성 키를 다른 내용에 사용했습니다.            |
| `aborted`            | 버전이 오래되었습니다.                                                           |
| `resource_exhausted` | 속도 제한 또는 리소스 제한이 적용됩니다.                                         |
| `unavailable`        | 의존 서비스나 데이터베이스가 실패했습니다. 변경이 적용되었는지는 알 수 없습니다. |

모든 응답과 DevBase가 직접 반환하는 모든 오류에는 추적 ID가 있습니다. 지원을 요청할 때 함께 알려 주세요. 256 KiB보다 큰 요청은 그보다 앞서 전송 계층이 `resource_exhausted`로 거부하며 추적 ID가 없으니 요청 크기를 줄이세요. 변경 요청에서 `unavailable`을 받았다면 같은 멱등성 키로 같은 요청을 반복해 적용 여부를 확인하세요.

## 테넌트 상태

DevBase는 무료입니다. 요금제, 잔액, 청구서는 사용을 제한하지 않습니다. 운영자의 차단과 테넌트 삭제만 제한할 수 있습니다.

- 테넌트가 차단되었거나 삭제 중이면 프로젝트 API 키는 동작하지 않습니다. Owner와 Admin은 여전히 프로젝트와 키를 읽고, 키를 폐기하고, 사용자와 프로젝트를 삭제할 수 있습니다. 새로 만들거나 변경할 수 없고, 키를 발급할 수 없으며, 사용자를 읽을 수 없습니다.
- DevBase가 테넌트 상태를 확인하지 못하면 추측하지 않고 `unavailable` 또는 `permission_denied`로 요청을 거부합니다.
- 테넌트가 영구 삭제되면 DevBase는 해당 프로젝트, 사용자, 키를 자동으로 삭제합니다.

## DevBase가 하지 않는 일

DevBase는 사용자를 로그인시키거나, 사용자의 Delino 계정을 만들거나, 사용자를 대신해 동작하지 않습니다. 프로젝트 간 사용자 공유, 테넌트 간 프로젝트 이동, 데이터 가져오기와 내보내기, 사용자 검색도 제공하지 않습니다. 백업이 있다면 DevBase가 활성 데이터에 적용하는 즉시 삭제의 범위 밖입니다.
