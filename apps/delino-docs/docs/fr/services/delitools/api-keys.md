# Clés API DeliTools

Les clés API permettent à vos serveurs d’appeler DeliTools sans jeton de connexion Delino. Delcore crée et vérifie les clés pour chaque service Delino ; le même mécanisme de clés fonctionne donc aussi pour les autres services. Vous gérez les clés avec votre jeton de connexion Delino et vous utilisez une clé comme jeton Bearer lorsque vous appelez un service.

## Types de clés

| Type                     | Agit au nom de                                | Qui peut la créer, la lister et la révoquer                        | Autorisations lors de l’utilisation                                                                                  |
| ------------------------ | --------------------------------------------- | ------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------- |
| Clé personnelle          | Vous, au sein d’un seul tenant                | Vous uniquement, et seulement tant que vous êtes membre du tenant. | Les droits accordés à la clé et vos autorisations actuelles dans le tenant. Les deux doivent autoriser l’appel.      |
| Clé de service du tenant | Le tenant, indépendamment de tout utilisateur | Un membre actuel du tenant ayant le rôle Owner ou Admin.           | Uniquement les droits accordés à la clé. Elle continue de fonctionner si la personne qui l’a créée quitte le tenant. |

- Les autres utilisateurs ne peuvent jamais voir ni révoquer vos clés personnelles, y compris ceux ayant les rôles Owner et Admin. Les rôles Billing et Member ne peuvent pas gérer les clés de service du tenant.
- Une identité machine-à-machine ne peut pas gérer les clés. Les clés ne servent jamais d’identifiants pour Delcore lui-même, mais uniquement pour les services auxquels elles donnent accès.
- Les deux types sont liés à un seul tenant. Une clé ne peut pas être utilisée pour un autre tenant.

## Créer une clé

Appelez `delcore.v1.DelcoreService/CreateApiKey` à l’adresse `https://delcore.api.delino.io` avec votre jeton de connexion Delino.

| Champ        | Règles                                                                                                                                                                                                |
| ------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `requestKey` | Un UUID v7 que vous générez. Répéter la requête avec la même clé et les mêmes données d’entrée renvoie le résultat initial.                                                                           |
| `tenantId`   | Le tenant auquel appartient la clé.                                                                                                                                                                   |
| `ownerKind`  | `API_KEY_OWNER_KIND_PERSONAL` ou `API_KEY_OWNER_KIND_TENANT`.                                                                                                                                         |
| `name`       | De 1 à 100 octets, non vide et non composé uniquement d’espaces, sans caractères de contrôle.                                                                                                         |
| `grants`     | De 1 à 16 attributions de droits. Chacune comporte un `serviceId` et de 1 à 32 `capabilities` de 100 octets maximum chacune. Un service n’apparaît qu’une fois et les capacités ne sont pas répétées. |
| `expiresAt`  | Date et heure futures facultatives au format RFC 3339. Sans ce champ, la clé reste valide jusqu’à sa révocation.                                                                                      |

Vous trouverez le `serviceId` et les capacités d’un service dans le catalogue Delcore (`GetCatalog`). Pour DeliTools, accordez les capacités `translate` et `extract` dont vous avez besoin. Delcore accepte uniquement les capacités déclarées par le service ; une capacité mal saisie est donc rejetée lors de la création de la clé.

```sh
curl --request POST "https://delcore.api.delino.io/delcore.v1.DelcoreService/CreateApiKey" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELINO_TOKEN" \
  --data '{"requestKey":"<uuid-v7>","tenantId":"<tenant-id>","ownerKind":"API_KEY_OWNER_KIND_TENANT","name":"Production translation","grants":[{"serviceId":"<deli-tools-service-id>","capabilities":["translate"]}],"expiresAt":"2027-01-01T00:00:00Z"}'
```

La réponse contient les détails de la clé et le `secret`, qui commence par `dlk_`.

- **Le secret n’apparaît qu’une seule fois.** Copiez-le dès maintenant dans votre gestionnaire de secrets. Delcore ne conserve qu’une empreinte de hachage et n’affiche jamais le secret dans les listes ni dans une réponse à une requête répétée.
- **Secret perdu.** Si vous perdez le secret, ou si la réponse à une requête répétée contient un `secret` vide, créez une nouvelle clé et révoquez l’ancienne.
- **Traitez-le comme un mot de passe.** Conservez-le dans le code côté serveur, et non dans les journaux, les dépôts ou les bundles frontend.
- Chaque tenant peut avoir un nombre limité de clés actives : 20 clés personnelles par utilisateur et 20 clés de tenant au total. Les clés révoquées et expirées ne sont pas comptabilisées.

Les droits accordés et la date d’expiration sont fixés lors de la création d’une clé. Il n’existe aucun appel de modification, de renouvellement ou de rotation. Pour modifier une clé, créez une clé de remplacement avec les nouveaux droits ou la nouvelle date d’expiration, configurez votre application pour l’utiliser, puis révoquez l’ancienne clé.

## Lister et révoquer les clés

- `ListApiKeys` prend les paramètres `tenantId` et `ownerKind`. Il renvoie les détails des clés, y compris les clés révoquées et expirées, mais jamais les secrets. Les listes de clés personnelles n’affichent que vos propres clés. Les listes de clés de tenant nécessitent le rôle Owner ou Admin. Utilisez `page.afterId` avec le `nextAfterId` renvoyé pour lire la suite.
- `RevokeApiKey` prend les paramètres `requestKey`, `tenantId` et `keyId`. La révocation prend effet immédiatement pour les nouvelles requêtes. Révoquer à nouveau une clé est sans risque.

## Utiliser une clé

Envoyez le secret sous la forme `Authorization: Bearer dlk_...` au service auquel la clé donne accès. Pour DeliTools, cela fonctionne pour `Translate` et `Extract`, comme décrit dans la [référence de l’API](/fr/services/delitools/api-reference).

- `tenantId` est facultatif avec une clé. Si vous l’envoyez, il doit correspondre au tenant de la clé.
- Une clé ne peut utiliser que les capacités que vous avez accordées pour ce service. Une clé destinée à un service ne fonctionne jamais pour un autre.
- Delcore vérifie la clé à chaque requête, puis à nouveau avant chaque nouvelle tentative d’IA. La révocation, l’expiration et la perte d’une autorisation ou de l’accès à la facturation bloquent les nouvelles requêtes et les tentatives supplémentaires. Un appel au fournisseur déjà en cours peut se terminer.
- Une clé personnelle cesse de fonctionner tant que vous n’êtes plus autorisé à utiliser la capacité dans le tenant, par exemple après votre retrait du tenant ou la perte d’une autorisation de groupe. Votre retrait ne supprime pas la clé.
- Une clé de tenant ne dépend pas de l’appartenance de son créateur au tenant.
- Les appels effectués avec une clé sont facturés au tenant de la clé aux mêmes tarifs que les appels effectués avec un jeton de connexion. Voir [Facturation](/fr/services/delitools/billing).
- Lorsque vous supprimez un tenant, Delcore supprime ses clés quand la suppression devient définitive. Les clés personnelles sont supprimées dès que le retrait de l’utilisateur est accepté. Une clé d’un tenant supprimé est invalide.

## Erreurs courantes

| Situation                                                                                 | Résultat                                                                                     |
| ----------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| Le secret est mal saisi, inconnu, révoqué ou expiré                                       | `unauthenticated`. Delcore ne précise pas lequel de ces cas s’applique.                      |
| La clé ne dispose pas du service ou de la capacité                                        | `permission_denied`                                                                          |
| `tenantId` ne correspond pas au tenant de la clé                                          | `permission_denied`                                                                          |
| Votre rôle ne permet pas de gérer ce type de clé                                          | `permission_denied` renvoyé par Delcore lors de la création, du listage ou de la révocation. |
| Vous révoquez la clé personnelle d’un autre utilisateur ou une clé inconnue               | `not_found` renvoyé par Delcore.                                                             |
| Vous créez une clé dans un tenant en cours de suppression ou au-delà de la limite de clés | `failed_precondition` renvoyé par Delcore.                                                   |
| Droits accordés, nom, service, capacité ou expiration non valides                         | `invalid_argument` renvoyé par Delcore lors de la création de la clé.                        |
| La facturation du tenant est bloquée                                                      | DeliTools renvoie `ERROR_REASON_BILLING_BLOCKED`.                                            |
