# DeliTools

DeliTools fournit des outils d’IA aux développeurs au moyen d’une API Connect. La version 1 propose deux méthodes synchrones : la **traduction** d’un tableau de textes et l’**extraction de données** à partir d’un texte vers du JSON conforme à votre schéma. Chaque appel renvoie le résultat final ou une erreur dans la même réponse. Il n’y a aucune tâche à interroger et aucun résultat à récupérer plus tard. Toutes les méthodes se trouvent sur `delitools.v1.DeliToolsService`.

Appelez DeliTools depuis du code côté serveur ou la ligne de commande, avec n’importe quel client Connect ou avec du JSON brut sur HTTP. Ne placez pas de clé d’API dans du code frontend. Définissez `DELITOOLS_URL` sur l’URL de base de votre serveur d’API DeliTools.

## Démarrage rapide

1. **Choisissez un identifiant.** Utilisez votre jeton de connexion Delino, ou créez une clé d’API avec `CreateKey`. Consultez la section Connexion et clés d’API.
2. **Trouvez l’identifiant de votre tenant.** L’utilisation est facturée au tenant Delino que vous indiquez dans `tenantId`.
3. **Appelez une méthode** avec `Authorization: Bearer <credential>`.

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

Dans JSON, les noms de champs utilisent lowerCamelCase et les valeurs d’énumération utilisent leur nom complet.

## Traduire du texte

`TranslateTextArray` traduit chaque chaîne d’un tableau.

| Champ            | Obligatoire | Signification                                                                                                                 |
| ---------------- | ----------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `tenantId`       | Oui         | Le tenant qui reçoit la facturation de l’utilisation. Utilisez son UUID version 7 canonique : en minuscules, avec des tirets. |
| `outputLanguage` | Oui         | Balise de langue BCP 47 de la traduction, par exemple `ko`, `pt-BR` ou `zh-Hans`.                                             |
| `inputLanguage`  | Non         | Balise de langue BCP 47 de la source. Omettez-la pour détecter une seule langue pour le tableau.                              |
| `model`          | Oui         | Un modèle figurant dans la liste d’autorisation de l’opérateur.                                                               |
| `texts`          | Oui         | Les chaînes à traduire.                                                                                                       |
| `idempotencyKey` | Non         | Consultez Idempotence.                                                                                                        |

La réponse contient `requestId` et `texts`, les traductions dans l’ordre d’origine et avec la longueur d’origine.

Règles :

- Les valeurs de langue sont des balises BCP 47. `outputLanguage` est obligatoire. Une balise qui n’est pas une balise BCP 47 valide, qui n’est pas en ASCII ou qui dépasse 64 octets renvoie une erreur d’entrée non valide.
- `texts` ne doit pas être vide, et aucune chaîne ne peut être vide ni ne contenir que des espaces.
- Si vous définissez `inputLanguage` et qu’elle est identique à `outputLanguage`, la requête n’est pas valide.
- Si vous omettez `inputLanguage`, DeliTools détecte une seule langue d’entrée pour l’ensemble du tableau. Un tableau multilingue est tout de même traduit. Si la langue détectée est égale à `outputLanguage`, le modèle traite quand même chaque élément. DeliTools ne renvoie pas le tableau d’origine sans traitement.
- Seuls les modèles figurant dans la liste d’autorisation de l’opérateur sont acceptés. Les autres modèles renvoient une erreur de modèle non valide. Un modèle qui existe dans le catalogue du Vercel AI Gateway n’est pas autorisé automatiquement.
- L’appel réussit uniquement si chaque élément réussit. L’échec d’un seul élément fait échouer toute la requête. DeliTools ne renvoie jamais de traductions partielles et ne remplace jamais le texte source pour un élément en échec.

## Extraire des données

`ExtractDataFromText` lit un texte et renvoie du JSON conforme à votre schéma JSON. Le service utilise un modèle fixe, `gpt-6-luna`. Vous ne choisissez pas le modèle.

| Champ            | Obligatoire | Signification                                                                                                                 |
| ---------------- | ----------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `tenantId`       | Oui         | Le tenant qui reçoit la facturation de l’utilisation. Utilisez son UUID version 7 canonique : en minuscules, avec des tirets. |
| `text`           | Oui         | Le texte source. Il ne doit être ni vide ni composé uniquement d’espaces.                                                     |
| `jsonSchema`     | Oui         | Un schéma JSON, sous forme de chaîne, qui utilise le sous-ensemble pris en charge.                                            |
| `idempotencyKey` | Non         | Consultez Idempotence.                                                                                                        |

La réponse contient `requestId` et `jsonData`, une chaîne JSON que DeliTools a validée par rapport à votre schéma.

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

Fonctionnement de l’extraction :

- Le modèle reçoit pour instruction d’utiliser uniquement les informations que le texte énonce. Si le texte ne contient pas des informations que le schéma exige, l’appel échoue avec une erreur d’extraction impossible. Le service n’invente pas de valeur pour satisfaire le schéma.
- DeliTools vérifie la sortie du modèle par rapport à votre schéma sur le serveur. Une sortie mal formée ou non conforme est un échec avec une erreur de sortie non valide. Elle n’est jamais renvoyée comme résultat, et DeliTools ne la réessaie ni ne la répare.
- Une vérification du schéma et une instruction ne garantissent pas que chaque valeur est sémantiquement correcte. Relisez les données extraites avant de vous y fier. DeliTools ne renvoie pas de preuve pour chaque valeur.
- DeliTools vérifie le schéma avant d’appeler un modèle d’IA. Un schéma en dehors du sous-ensemble pris en charge renvoie une erreur de schéma non valide, sans appel d’IA et sans facturation.

### Sous-ensemble de schéma

DeliTools accepte uniquement la partie de JSON Schema que prend en charge la sortie structurée du modèle d’extraction.

Pris en charge :

- Un objet à la racine.
- Les types `object`, `array`, `string`, `number`, `integer`, `boolean` et `null`, imbriqués jusqu’à une profondeur de 8 niveaux. Chaque nœud du schéma a exactement un `type`, écrit sous forme de chaîne. Un tableau de types, comme `["string", "null"]`, est rejeté.
- `properties`, avec chaque propriété listée dans `required`. Le nom d’une propriété comporte de 1 à 256 caractères.
- `additionalProperties` défini sur `false` pour chaque objet.
- `items` pour les tableaux.
- `description` sur n’importe quel schéma.
- `enum` avec des valeurs scalaires. Une énumération ne doit pas être vide, ses valeurs doivent être uniques et correspondre au `type` du nœud, et une énumération `integer` n’accepte que des nombres entiers.

Non pris en charge, et rejeté avant l’exécution :

- Tout mot-clé absent de la liste ci-dessus, par exemple `pattern`, `minimum`, `format` ou `oneOf`.
- Les propriétés facultatives. Chaque propriété d’un objet doit figurer dans `required`. `required` doit lister chaque propriété exactement une fois et ne doit citer aucune propriété que `properties` ne définit pas.
- `additionalProperties` défini sur `true`, ou absent.
- Les références : `$ref` et les schémas récursifs.
- Les schémas qui dépassent les limites du fournisseur : 4 998 propriétés au total, 1 000 valeurs d’énumération, ou 119 980 caractères pour les noms de propriétés et les valeurs d’énumération réunis. Le fournisseur autorise 5 000 propriétés et 120 000 caractères au total, et DeliTools réserve le reste pour son enveloppe de réponse. Une énumération de plus de 250 valeurs peut contenir au plus 15 000 caractères de valeurs de type chaîne.

Un schéma pris en charge :

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

Un schéma non pris en charge, car `note` est facultatif et `pattern` n’est pas pris en charge :

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

## Comportement des requêtes

- Les deux méthodes sont synchrones. Gardez la connexion ouverte jusqu’à ce que vous receviez un résultat ou une erreur.
- Chaque succès et chaque erreur contiennent un identifiant de requête, un UUID version 7. Incluez-le lorsque vous demandez de l’assistance. Les réponses de succès contiennent uniquement le résultat et l’identifiant de requête. Elles ne contiennent ni le modèle, ni le nombre de jetons, ni le montant facturé. Consultez Facturation pour examiner l’utilisation.
- DeliTools réessaie les erreurs transitoires du fournisseur d’IA, dans une limite définie par l’opérateur. Il ne réessaie pas une sortie mal formée et ne bascule jamais de lui-même vers un autre modèle.
- Si vous annulez l’appel ou vous déconnectez, DeliTools tente d’annuler l’appel d’IA et arrête les nouvelles tentatives. L’utilisation qui a déjà eu lieu est quand même mesurée et facturée.
- L’opérateur définit les limites de taille d’entrée, de nombre d’éléments à traduire, de délai d’exécution, de débit de requêtes par tenant et de requêtes simultanées par tenant. Il s’agit de paramètres de déploiement, et non de limites fixes du produit. Une requête qui dépasse une limite renvoie une erreur de limite.

## Connexion et clés d’API

Envoyez `Authorization: Bearer <credential>` avec chaque requête. Deux types d’identifiants fonctionnent pour les deux outils :

- **Un jeton de connexion Delino** (un jeton d’accès utilisateur Logto) agit en tant qu’utilisateur connecté.
- **Une clé d’API DeliTools** agit selon ce que décrit son type de clé.

Chaque requête est également vérifiée par rapport à votre tenant dans Delcore. Le tenant doit être actif et autorisé à exécuter des tâches payantes, et la facturation doit l’autoriser. Si DeliTools ne peut pas effectuer ces vérifications, il rejette les nouvelles exécutions payantes.

### Types de clés et capacités

- Une **clé personnelle** appartient à un utilisateur. Ses autorisations correspondent à l’intersection de ses capacités et des autorisations actuelles de cet utilisateur. Lorsque l’utilisateur perd l’accès au tenant, la clé cesse de fonctionner.
- Une **clé de service** appartient au tenant. Elle ne dépend pas de l’appartenance de l’utilisateur qui l’a créée. Seul un Owner ou Admin du tenant peut créer et gérer des clés de service.
- Chaque clé possède l’une des capacités ou les deux : `CAPABILITY_TRANSLATOR` et `CAPABILITY_EXTRACTOR`. Une clé ne peut appeler que les outils correspondant à ses capacités.
- Les utilisateurs gèrent leurs propres clés personnelles. La gestion des clés nécessite un jeton de connexion Delino. Une clé d’API ne peut ni créer ni révoquer de clés.

### Gérer les clés

- `CreateKey` : envoyez `tenantId`, `kind` (`KEY_KIND_PERSONAL` ou `KEY_KIND_SERVICE`), `capabilities` et un `expiresAt` facultatif.
- `ListKeys` : répertorie les clés d’un tenant, filtrées par `kind`, avec `afterId` et `pageSize` pour la pagination.
- `RevokeKey` : envoyez `tenantId` et `keyId`.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DeliToolsService/CreateKey" \
  --header "Content-Type: application/json" \
  --header "Connect-Protocol-Version: 1" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"tenantId":"<tenant-id>","kind":"KEY_KIND_SERVICE","capabilities":["CAPABILITY_TRANSLATOR"]}'
```

- **Secret à usage unique.** La réponse de `CreateKey` contient le `secret` une seule fois. DeliTools stocke uniquement une empreinte de vérification et ne peut pas afficher à nouveau le secret. Si vous perdez une clé, créez-en une nouvelle. Conservez le secret dans un gestionnaire de secrets.
- **Expiration.** `expiresAt` est facultatif. Une clé sans expiration reste valide jusqu’à sa révocation. Une clé personnelle exige aussi que son propriétaire conserve l’accès au tenant. Une clé de service ne dépend pas de l’utilisateur qui l’a créée : retirer cet utilisateur ne l’arrête donc pas. Révoquez la clé de service ou donnez-lui une expiration.
- **Révocation.** Après la révocation d’une clé, ou après la perte d’une autorisation par l’utilisateur d’une clé personnelle ou d’un jeton de connexion, les nouvelles requêtes sont rejetées et un appel en cours n’effectue plus de nouvelle tentative d’IA. Un appel déjà en cours peut se terminer. Son utilisation est facturée.

## Facturation

DeliTools signale l’utilisation à Delcore. Delcore la tarifie et la règle en USD.

- **Base.** La base correspond aux prix publics par jeton du modèle dans le catalogue de modèles du Vercel AI Gateway, multipliés par les jetons que l’appel a réellement mesurés. Les remises du Gateway et les crédits promotionnels ne réduisent pas la base.
- **Majoration.** La traduction coûte la base multipliée par 1.15. L’extraction coûte la base multipliée par 1.30. Les montants utilisent des décimaux exacts et les règles d’arrondi de Delcore.
- **Changements de prix.** DeliTools met à jour automatiquement les prix du catalogue. Un nouveau prix s’applique à tous les clients pour les requêtes qui démarrent après son entrée en vigueur. Une requête conserve le prix de son démarrage pour toutes ses nouvelles tentatives. Si DeliTools n’a pas de prix pour un modèle, il rejette les nouvelles exécutions d’IA.
- **Les échecs sont facturables.** Les jetons utilisés par les nouvelles tentatives, les appels en échec, les délais dépassés, les annulations et les résultats que vous n’avez pas pu recevoir comptent quand même, avec la majoration.
- **Utilisation inconnue.** Si DeliTools ne peut pas lire immédiatement l’utilisation d’un appel, il reporte la facturation et effectue le rapprochement avec le Gateway plus tard. Il n’estime jamais l’utilisation pour une facturation. Si l’utilisation est toujours inconnue lorsque l’enregistrement expire après 30 jours, cette facturation est abandonnée.
- **Où consulter l’utilisation.** Les réponses ne contiennent aucune information d’utilisation. Utilisez l’interface de facturation Delino de votre tenant pour examiner l’utilisation et les frais.
- **Tenants bloqués.** Si la facturation bloque votre tenant, les nouvelles exécutions échouent avec une erreur de facturation bloquée. Un appel déjà en cours n’est pas interrompu. Comme DeliTools mesure l’utilisation après l’exécution, des appels simultanés peuvent dépasser un solde ou une limite avant que le blocage ne s’applique.

## Idempotence

`idempotencyKey` est facultatif. Si vous l’envoyez, elle doit comporter de 1 à 256 caractères ASCII visibles (ni espaces ni caractères de contrôle) ; toute autre clé renvoie une erreur d’entrée non valide. Sans cette clé, chaque appel est une nouvelle exécution et peut créer une nouvelle facturation, y compris lorsque vous réessayez après une défaillance du réseau.

Avec une clé, DeliTools suit la requête par **tenant, méthode et clé**. Cela s’applique à différents identifiants du même tenant. DeliTools vérifie votre autorisation actuelle avant de vous indiquer l’état d’une requête.

| Situation                                                   | Résultat                                                                                           |
| ----------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| Nouvelle clé                                                | L’appel s’exécute normalement.                                                                     |
| Même clé et même entrée, encore en cours                    | Une erreur de traitement en cours avec l’identifiant de requête du premier appel. Il n’attend pas. |
| Même clé et même entrée, terminé                            | Une erreur de requête déjà traitée. Le résultat n’est pas rejoué.                                  |
| Même clé et entrée différente                               | Une erreur de conflit d’entrée.                                                                    |
| Le premier appel s’est terminé dans un état inconnu         | Une erreur de résultat inconnu. DeliTools ne répète pas l’appel d’IA de lui-même.                  |
| Même clé après l’expiration de l’enregistrement de 30 jours | Un nouvel appel, facturé normalement.                                                              |

DeliTools conserve la clé, une empreinte de l’entrée et l’état. Il ne conserve ni l’entrée ni le résultat. La déduplication ne vaut que pendant 30 jours après le premier appel. Enregistrez le résultat lorsque vous le recevez : un appel répété avec la même clé ne peut pas le renvoyer à nouveau.

## Erreurs

Chaque erreur possède un code Connect, une raison stable, un identifiant de requête et un message en anglais. La raison et l’identifiant de requête se trouvent dans le détail d’erreur `delitools.v1.ErrorDetail`. Utilisez la raison, et non le message, dans votre code.

| Raison                               | Code Connect          | Signification                                                                                                                       |
| ------------------------------------ | --------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `ERROR_REASON_INVALID_INPUT`         | `invalid_argument`    | La requête n’est pas valide, par exemple un tableau vide ou des langues identiques.                                                 |
| `ERROR_REASON_INVALID_SCHEMA`        | `invalid_argument`    | Le schéma est en dehors du sous-ensemble pris en charge.                                                                            |
| `ERROR_REASON_INVALID_MODEL`         | `invalid_argument`    | Le modèle ne figure pas dans la liste d’autorisation.                                                                               |
| `ERROR_REASON_UNAUTHENTICATED`       | `unauthenticated`     | L’identifiant est absent, invalide, expiré ou révoqué.                                                                              |
| `ERROR_REASON_FORBIDDEN`             | `permission_denied`   | L’identifiant ou l’utilisateur n’a pas le droit d’utiliser cet outil dans ce tenant.                                                |
| `ERROR_REASON_BILLING_BLOCKED`       | `failed_precondition` | La facturation n’autorise pas de nouvelle exécution payante pour le tenant.                                                         |
| `ERROR_REASON_PRICE_UNAVAILABLE`     | `failed_precondition` | Aucun prix public n’est disponible pour le modèle, donc DeliTools ne l’exécute pas.                                                 |
| `ERROR_REASON_LIMIT`                 | `resource_exhausted`  | Une limite configurée a été dépassée, comme la taille d’entrée, le nombre d’éléments, le débit ou la simultanéité.                  |
| `ERROR_REASON_PROCESSING`            | `aborted`             | Une requête avec la même clé d’idempotence est toujours en cours.                                                                   |
| `ERROR_REASON_ALREADY_PROCESSED`     | `already_exists`      | Une requête avec la même clé d’idempotence et la même entrée est déjà terminée.                                                     |
| `ERROR_REASON_INPUT_CONFLICT`        | `failed_precondition` | La clé d’idempotence a été utilisée avec une entrée différente.                                                                     |
| `ERROR_REASON_UNKNOWN_OUTCOME`       | `unknown`             | Le résultat de la première exécution est inconnu.                                                                                   |
| `ERROR_REASON_EXTRACTION_IMPOSSIBLE` | `failed_precondition` | Le texte ne contient pas d’informations que le schéma exige.                                                                        |
| `ERROR_REASON_INVALID_OUTPUT`        | `data_loss`           | Le modèle a renvoyé une sortie mal formée ou une sortie qui ne correspond pas au schéma.                                            |
| `ERROR_REASON_PROVIDER`              | `unavailable`         | Le fournisseur d’IA a échoué après les nouvelles tentatives autorisées.                                                             |
| `ERROR_REASON_TIMEOUT`               | `deadline_exceeded`   | Le délai d’exécution est dépassé.                                                                                                   |
| `ERROR_REASON_CANCELED`              | `canceled`            | L’appel a été annulé.                                                                                                               |
| `ERROR_REASON_INTERNAL`              | `internal`            | Une défaillance interne, par exemple du stockage.                                                                                   |
| `ERROR_REASON_INTERNAL`              | `unavailable`         | La vérification d’autorisation ou de facturation du tenant n’a pas pu aboutir ; les nouveaux traitements payants sont donc rejetés. |

Traitement :

- Corrigez la requête pour les erreurs `invalid_argument`, `permission_denied`, `unauthenticated` et `failed_precondition` avant de réessayer. Levez le blocage de facturation pour une erreur de facturation bloquée.
- Une erreur de fournisseur, de délai dépassé ou interne peut être réessayée plus tard. Les échecs de fournisseur et de délai dépassé peuvent quand même être facturés. Utilisez une clé d’idempotence pour éviter une seconde exécution lorsque la première a pu réussir. Une erreur `unavailable` avec `ERROR_REASON_INTERNAL` signifie que DeliTools n’a pas pu terminer la vérification d’autorisation ou de facturation. Tenez compte à la fois du code et de la raison, car `ERROR_REASON_INTERNAL` apparaît aussi avec `internal`.
- Les erreurs de résultat inconnu, de traitement en cours et de requête déjà traitée contiennent l’identifiant de requête du premier appel. Contactez l’assistance avec cet identifiant si le résultat reste inconnu.

## Conservation des données

DeliTools ne stocke jamais votre texte d’entrée, votre schéma ni les résultats. Il les exclut également des journaux, des traces et des événements d’audit. Les identifiants et les secrets sont exclus de la télémétrie.

DeliTools stocke uniquement les données dont il a besoin pour exécuter le service et le facturer : les empreintes de vérification des clés d’API, l’utilisateur et le tenant auxquels appartient une requête, les empreintes d’entrée pour l’idempotence, les états des requêtes, les identifiants de tentatives, l’utilisation mesurée en jetons, les instantanés de prix, ainsi que des enregistrements d’audit et d’exploitation sûrs.

| Données                                                             | Conservées pendant                                 |
| ------------------------------------------------------------------- | -------------------------------------------------- |
| Enregistrements d’idempotence                                       | 30 jours à compter de la première création         |
| Enregistrements de livraison et de rapprochement de l’utilisation   | 30 jours à compter de la première création         |
| Enregistrements d’audit de la plateforme et journaux d’exploitation | 30 jours à compter de la création                  |
| Clés d’API                                                          | Non supprimées par la conservation de 30 jours     |
| Écritures comptables acceptées dans Delcore                         | Selon les règles de conservation propres à Delcore |

La conservation de 30 jours ne s’applique pas aux clés d’API. Le nettoyage de conservation ne les supprime jamais. L’expiration et la révocation empêchent seulement une clé de s’authentifier ; son hachage de vérification et son attribution restent donc stockés.

Les 30 jours commencent lorsque l’enregistrement est créé pour la première fois. Les nouvelles tentatives et les changements d’état ne les prolongent pas. DeliTools supprime un enregistrement à son expiration, quel que soit son état, y compris les requêtes qui ne se sont pas terminées et l’utilisation qui n’est pas encore livrée. Vous acceptez deux risques :

- Après l’expiration d’un enregistrement d’idempotence, un appel répété avec la même clé s’exécute et est de nouveau facturé.
- L’utilisation qui est encore en attente lorsque son enregistrement expire est perdue, et DeliTools ne la facture pas.

## Traitement des données par les fournisseurs externes

Pour effectuer son travail, DeliTools envoie votre texte, et pour l’extraction votre schéma, au Vercel AI Gateway, puis de là au fournisseur du modèle qui exécute votre requête. Ces services traitent les données selon leurs propres politiques. Lisez-les avant d’envoyer des données à DeliTools :

- [Vercel AI Gateway zero data retention](https://vercel.com/docs/ai-gateway/security-and-compliance/zdr)
- [Vercel AI Gateway models](https://vercel.com/ai-gateway/models) : la page de chaque modèle liste ses fournisseurs.
- La politique de données du fournisseur du modèle que vous utilisez.

DeliTools ne promet aucune conservation nulle des données chez aucun fournisseur. Il ne détecte pas les données sensibles et ne les supprime pas de votre entrée. Ne soumettez pas de données que vous n’avez pas le droit de partager avec ces fournisseurs.

## Limites

L’opérateur du serveur d’API DeliTools définit les limites de taille d’entrée, de nombre d’éléments d’une traduction, de délai d’exécution, de requêtes par minute par tenant, de requêtes simultanées par tenant, de nombre de nouvelles tentatives d’IA et la liste d’autorisation des modèles de traduction. Il s’agit de paramètres de déploiement qui peuvent changer. Ce ne sont pas des limites fixes du produit, et DeliTools ne donne aucune garantie de débit ni de disponibilité. Une requête qui dépasse une limite renvoie `ERROR_REASON_LIMIT` ou, pour un modèle non valide, `ERROR_REASON_INVALID_MODEL`. Demandez à l’opérateur de votre déploiement ses valeurs actuelles.
