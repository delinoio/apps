# Référence de l’API DeliTools

Cette page décrit les deux méthodes de DeliTools. Toutes deux sont des procédures Connect sur `delitools.v1.DelitoolsService` et acceptent du JSON via HTTP `POST`. Les noms de champs sont en lowerCamelCase dans le JSON. Envoyez `Content-Type: application/json` et `Authorization: Bearer <credential>`. Consultez [DeliTools](/fr/services/delitools/) pour les identifiants et les tenants.

Chaque réponse inclut un identifiant de requête, dans le corps et dans l’en-tête `X-Delitools-Request-Id`. Les réponses n’incluent ni l’utilisation des tokens ni les frais. Consultez ces informations dans Delcore ; voir [Facturation](/fr/services/delitools/billing).

Chaque appel constitue une nouvelle exécution. DeliTools ne dispose ni de clé d’idempotence ni de résultats stockés. Si vous répétez une requête, elle s’exécute à nouveau et peut être facturée à nouveau.

## Translate

`POST /delitools.v1.DelitoolsService/Translate`

| Champ            | Type               | Règles                                                                                                                       |
| ---------------- | ------------------ | ---------------------------------------------------------------------------------------------------------------------------- |
| `tenantId`       | chaîne             | Obligatoire pour un jeton de connexion Delino. Pour une clé API, vide ou égal au tenant de la clé.                           |
| `texts`          | tableau de chaînes | Obligatoire. Au moins un élément. Aucune chaîne vide. Le nombre d’éléments et la taille totale sont limités par l’opérateur. |
| `targetLanguage` | chaîne             | Obligatoire. Une étiquette de langue BCP 47, comme `fr` ou `pt-BR`.                                                          |
| `sourceLanguage` | chaîne             | Facultatif. Une étiquette de langue BCP 47. Si elle est omise, le modèle détecte la langue source.                           |
| `model`          | chaîne             | Obligatoire. Un identifiant de modèle autorisé par l’opérateur.                                                              |

La réponse contient `requestId` et `translations`. `translations` contient le même nombre d’éléments, dans le même ordre, que `texts`.

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

- **Étiquettes de langue.** DeliTools vérifie qu’une étiquette est bien formée selon BCP 47, sans tenir compte de la casse, avec au maximum 64 caractères et une langue principale de deux ou trois lettres. Les étiquettes composées uniquement de sous-étiquettes à usage privé et les étiquettes héritées sont rejetées. DeliTools ne vérifie pas que le modèle prend en charge la langue.
- **Tout ou rien.** Le résultat est atomique. Si un élément échoue, toute la requête échoue et vous ne recevez aucune traduction partielle.
- **Aucun cas particulier.** Une langue source omise, une entrée multilingue et un texte déjà dans la langue cible sont tous transmis au modèle. DeliTools ne les rejette pas et ne copie pas le texte source à votre place.
- **Modèle.** Seuls les modèles autorisés par l’opérateur et dont les tarifs sont enregistrés dans Delcore fonctionnent. Aucun autre modèle n’est jamais substitué. Demandez à l’opérateur du service quels identifiants de modèle sont disponibles.

## Extract

`POST /delitools.v1.DelitoolsService/Extract`

| Champ        | Type   | Règles                                                                                                                              |
| ------------ | ------ | ----------------------------------------------------------------------------------------------------------------------------------- |
| `tenantId`   | chaîne | Obligatoire pour un jeton de connexion Delino. Pour une clé API, vide ou égal au tenant de la clé.                                  |
| `text`       | chaîne | Obligatoire et non vide. La taille est limitée par l’opérateur.                                                                     |
| `jsonSchema` | chaîne | Obligatoire. Un document JSON Schema, sous forme de chaîne JSON, qui utilise uniquement le sous-ensemble pris en charge ci-dessous. |

Le modèle est fixé à `openai/gpt-6-luna`. Vous ne pouvez pas le choisir. La réponse contient `requestId` et `json`, une chaîne JSON conforme à votre schéma.

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

- **Fondé sur votre texte.** Le modèle doit extraire les valeurs du texte. Si le texte ne contient pas les informations requises, la requête échoue avec `ERROR_REASON_EXTRACTION_FAILED`. DeliTools n’invente pas de valeurs.
- **Sortie vérifiée.** DeliTools valide le résultat par rapport à votre schéma. Une sortie mal formée ou non conforme échoue avec `ERROR_REASON_INVALID_OUTPUT`. DeliTools ne réessaie pas pour corriger une sortie invalide.
- **Exactitude.** Un résultat conforme au schéma peut tout de même être erroné. Vérifiez les résultats importants.
- **Les schémas non pris en charge échouent en premier.** DeliTools rejette un schéma qui sort du sous-ensemble pris en charge avant d’exécuter le modèle ; une telle requête n’est donc pas facturée.

### Sous-ensemble de JSON Schema pris en charge

Le schéma est interprété selon JSON Schema 2020-12. Il doit s’agir d’un objet JSON. `$schema` est facultatif et autorisé uniquement à la racine, où DeliTools ignore sa valeur. Chaque schéma, y compris la racine, chaque schéma de propriété et le schéma `items`, doit comporter `type`, `enum` ou `const`. Un schéma peut avoir au maximum 8 niveaux de profondeur et 256 nœuds de schéma, et sa taille est limitée par l’opérateur.

| Mot-clé                  | Règle                                                                                                                                                                                    |
| ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `type`                   | `object`, `array`, `string`, `number`, `integer`, `boolean`, `null`, ou un tableau de ces types, par exemple `["string","null"]` pour une valeur pouvant être nulle.                     |
| `properties`, `required` | `properties` associe des noms à des schémas. Chaque nom de `required` doit figurer dans `properties`. Au maximum 64 propriétés par objet, et les noms de `required` ne sont pas répétés. |
| `additionalProperties`   | `true` ou `false` uniquement (pas un schéma).                                                                                                                                            |
| `items`                  | Un schéma pour tous les éléments.                                                                                                                                                        |
| `enum`, `const`          | `enum` contient de 1 à 64 valeurs distinctes. `const` est une valeur unique.                                                                                                             |
| `minLength`, `maxLength` | Entiers de 0 à 1 000 000, pour les chaînes. Le minimum ne peut pas dépasser le maximum.                                                                                                  |
| `minItems`, `maxItems`   | Entiers de 0 à 1 000 000, pour les tableaux. Le minimum ne peut pas dépasser le maximum.                                                                                                 |
| `minimum`, `maximum`     | Nombres finis, pour `number` et `integer`. Le minimum ne peut pas dépasser le maximum.                                                                                                   |
| `title`, `description`   | Notes destinées au modèle. Elles ne modifient pas la validation.                                                                                                                         |

Les éléments suivants ne sont pas pris en charge et renvoient `ERROR_REASON_UNSUPPORTED_SCHEMA` : `$ref`, `$defs`, `definitions`, `$id`, `$comment`, `oneOf`, `anyOf`, `allOf`, `not`, `if`/`then`/`else`, `patternProperties`, `propertyNames`, `pattern`, `format`, `uniqueItems`, `multipleOf`, `exclusiveMinimum`, `exclusiveMaximum`, `contains`, `prefixItems`, `default`, les mots-clés `unevaluated`, les schémas booléens, les mots-clés inconnus et les schémas trop volumineux ou trop profondément imbriqués. DeliTools ne suit jamais les références ; un schéma ne peut donc pas lui faire récupérer quoi que ce soit.

Définissez `"additionalProperties": false` sur les objets lorsque vous souhaitez uniquement les propriétés que vous avez répertoriées.

## Erreurs

Les erreurs utilisent un code Connect standard. Les détails de l’erreur incluent un motif stable et l’identifiant de requête. Les messages sont en anglais et ne contiennent jamais votre contenu.

| Motif                                 | Code Connect                                | Signification                                                                                                                                                                                                                                        |
| ------------------------------------- | ------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ERROR_REASON_INVALID_INPUT`          | `invalid_argument`                          | Un champ est invalide : tableau ou chaîne vide, étiquette de langue incorrecte, tenant manquant ou mal formé, modèle manquant ou non autorisé, ou schéma qui n’est pas du JSON valide.                                                               |
| `ERROR_REASON_UNAUTHENTICATED`        | `unauthenticated`                           | L’identifiant est manquant, mal formé, expiré, révoqué ou inconnu.                                                                                                                                                                                   |
| `ERROR_REASON_PERMISSION_DENIED`      | `permission_denied`                         | L’identifiant ne dispose pas de la capacité, de l’autorisation accordée à la clé ou de votre permission sur le tenant, ou le `tenantId` envoyé n’est pas le tenant de la clé.                                                                        |
| `ERROR_REASON_BILLING_BLOCKED`        | `failed_precondition`                       | Delcore a bloqué l’utilisation payante, par exemple en raison du solde, d’un renouvellement, d’un litige, d’un blocage par l’opérateur ou d’un tenant en cours de suppression.                                                                       |
| `ERROR_REASON_LIMIT_EXCEEDED`         | `resource_exhausted`                        | L’entrée, le nombre d’éléments ou le schéma dépasse les limites de l’opérateur.                                                                                                                                                                      |
| `ERROR_REASON_RATE_LIMITED`           | `resource_exhausted`                        | Votre tenant a atteint sa limite de fréquence des requêtes ou d’exécutions simultanées. Réessayez plus tard. Une erreur de fréquence des requêtes inclut un en-tête `Retry-After`.                                                                   |
| `ERROR_REASON_MODEL_UNAVAILABLE`      | `invalid_argument` ou `failed_precondition` | Le modèle n’est pas autorisé (`invalid_argument`), ou aucun tarif d’entrée et de sortie n’est enregistré pour lui (`failed_precondition`). Aucun autre modèle n’est substitué.                                                                       |
| `ERROR_REASON_UNSUPPORTED_SCHEMA`     | `invalid_argument`                          | Le schéma utilise un mot-clé hors du sous-ensemble pris en charge.                                                                                                                                                                                   |
| `ERROR_REASON_PROVIDER_FAILED`        | `unavailable` ou `internal`                 | Le fournisseur d’IA a continué à échouer après les nouvelles tentatives autorisées (`unavailable`), ou il a rejeté la requête (`internal`).                                                                                                          |
| `ERROR_REASON_INVALID_OUTPUT`         | `internal`                                  | La sortie du modèle était mal formée, tronquée ou non conforme au contrat.                                                                                                                                                                           |
| `ERROR_REASON_EXTRACTION_FAILED`      | `failed_precondition`                       | Le texte ne contient pas les informations requises.                                                                                                                                                                                                  |
| `ERROR_REASON_TIMEOUT`                | `deadline_exceeded`                         | Le délai d’exécution de l’opérateur a expiré. Il couvre les vérifications et toutes les tentatives.                                                                                                                                                  |
| `ERROR_REASON_CANCELED`               | `canceled`                                  | La requête a été annulée ou la connexion fermée.                                                                                                                                                                                                     |
| `ERROR_REASON_DEPENDENCY_UNAVAILABLE` | `unavailable`                               | Une vérification d’autorisation, de tarif ou de facturation n’a pas pu être effectuée, ou DeliTools n’a pas pu enregistrer l’utilisation de l’appel. L’utilisation payante est bloquée par défaut en cas d’échec, et vous ne recevez aucun résultat. |

Une requête qui dépasse la limite globale de taille des requêtes de l’opérateur peut recevoir une simple erreur `resource_exhausted` sans ces détails. Consultez [Dépannage](/fr/services/delitools/troubleshooting) pour savoir quoi faire.

## Limites et nouvelles tentatives

L’opérateur configure les limites suivantes, qui peuvent changer : taille de l’entrée, nombre d’éléments à traduire, taille et profondeur du schéma, délai d’exécution, fréquence des requêtes et rafale par tenant, exécutions simultanées par tenant et nombre maximal de nouvelles tentatives en cas d’erreurs transitoires. Contactez l’opérateur du service pour connaître les valeurs qui s’appliquent à vous. Une limite est une protection, pas une garantie de débit.

- DeliTools ne réessaie qu’en cas de défaillances transitoires du fournisseur d’IA, telles que les erreurs réseau, les erreurs serveur et les réponses de limitation de fréquence, dans la limite du nombre configuré. Il ne réessaie jamais pour corriger une sortie invalide.
- Avant chaque nouvelle tentative, DeliTools vérifie à nouveau votre autorisation. Si votre clé a été révoquée ou a expiré, si vous avez perdu votre permission ou si la facturation est bloquée, DeliTools s’arrête et renvoie l’erreur correspondante. Un appel au fournisseur déjà en cours peut se terminer.
- Si vous annulez la requête ou fermez la connexion, DeliTools annule l’appel au fournisseur et cesse de réessayer.
- Si votre client ferme l’appel ou si son propre délai d’attente y met fin, DeliTools traite cela comme une annulation.
- Le délai d’exécution de l’opérateur couvre l’autorisation, la validation et chaque tentative. Lorsqu’il expire, DeliTools interrompt l’appel au fournisseur et renvoie `ERROR_REASON_TIMEOUT`.
