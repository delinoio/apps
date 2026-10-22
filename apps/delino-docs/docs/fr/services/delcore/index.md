# Delcore

Delcore fournit des tenants partagés, le contrôle d’accès et la facturation pour les services Delino. Le serveur API officiel est `https://delcore.api.delino.io`. Cette page décrit la vérification des clés API, la tarification de l’utilisation, l’expiration des enregistrements et la vérification de l’état des tenants à l’intention des développeurs de services.

## Vérifier l’état d’un tenant

Un service enregistré peut demander à Delcore si un tenant est actif, bloqué ou supprimé. Utilisez cette vérification lorsque votre service stocke des données pour un tenant, mais ne dispose pas d’un utilisateur Delino pour chaque requête, par exemple lorsqu’une clé API de projet effectue l’appel.

Appelez `delcore.v1.DelcoreService/GetTenantStates` avec 1 à 100 identifiants de tenant. Chaque identifiant doit être un UUID version 7 en minuscules. Authentifiez-vous avec le jeton d’accès de l’application machine-to-machine Logto de votre service. Le service doit être enregistré auprès de Delcore et disposer de la portée `authorize`. Les jetons utilisateur, les services non enregistrés et les services dépourvus de cette portée ne peuvent pas lire l’état des tenants.

La réponse reprend chaque identifiant de tenant demandé, dans l’ordre où vous les avez envoyés, avec un état :

| État                    | Signification                                                   |
| ----------------------- | --------------------------------------------------------------- |
| `TENANT_STATE_ACTIVE`   | Le tenant existe et n’est ni bloqué ni en cours de suppression. |
| `TENANT_STATE_BLOCKED`  | Un opérateur de la plateforme a bloqué le tenant.               |
| `TENANT_STATE_DELETING` | La suppression est en attente.                                  |
| `TENANT_STATE_DELETED`  | La suppression est définitive.                                  |
| `TENANT_STATE_UNKNOWN`  | Delcore n’a aucun tenant portant cet identifiant.               |

Lorsque plusieurs conditions s’appliquent, `DELETED` prévaut sur `DELETING`, `DELETING` prévaut sur `BLOCKED`, et `BLOCKED` prévaut sur `ACTIVE`.

L’état ne dépend pas de la facturation. Un abonnement expiré, un échec de renouvellement, un litige ou un solde faible ne font pas passer un tenant à l’état `BLOCKED`. Pour déterminer si un utilisateur peut exécuter des tâches payantes, utilisez plutôt `Authorize`. La vérification ne fait que lire des données. Elle ne modifie pas de tenant, n’autorise pas d’utilisation payante, ne contacte pas de prestataire de paiement et ne renvoie aucun nom de tenant, détail utilisateur ou renseignement de facturation.

## Gérer les résultats et les échecs

- N’autorisez l’accès que pour `TENANT_STATE_ACTIVE`, ou lorsque vos propres règles documentées pour les propriétaires ou les administrateurs permettent l’accès à un tenant bloqué ou en cours de suppression.
- Traitez `TENANT_STATE_UNKNOWN`, les erreurs de requête, les dépassements de délai et l’indisponibilité de Delcore comme un refus d’accès. Ne supprimez pas de données dans ces cas.
- Pour le nettoyage lié à la suppression d’un tenant, ne supprimez automatiquement les données de tenant que vous conservez qu’après un état explicite `TENANT_STATE_DELETED`. `BLOCKED` et `DELETING` n’autorisent jamais ce nettoyage. L’expiration à échéance fixe des enregistrements s’applique indépendamment de l’état du tenant.
- Des identifiants invalides, une requête vide ou plus de 100 identifiants renvoient une erreur d’argument non valide. Des informations d’authentification absentes, expirées ou invalides renvoient une erreur de non-authentification. Un appelant qui n’est pas autorisé à lire l’état des tenants reçoit une erreur de permission refusée. Les autres échecs renvoient une erreur avec un identifiant de corrélation ; incluez cet identifiant, présent dans les détails de l’erreur et dans l’en-tête `X-Delcore-Correlation-Id`, lorsque vous demandez de l’aide.
- Réessayez ultérieurement une requête ayant échoué. L’échec d’une requête ne signifie jamais qu’un tenant est actif ou supprimé.

## Tarifs d’utilisation et expiration des enregistrements

Les prix unitaires d’utilisation s’appliquent au début de l’exécution initiale pour chaque client, y compris les abonnés existants. Indiquez ce début dans `IngestEvent` sous la forme de `occurred_at` et conservez-le lors des nouvelles tentatives d’exécution et du rapprochement de l’utilisation. Un changement de prix s’applique aux exécutions ultérieures ; les envois tardifs et les corrections conservent l’attribution tarifaire d’origine. Les tarifs des paiements récurrents, les fonctionnalités sélectionnées et les allocations d’utilisation incluse conservent les conditions de leur abonnement.

La déduplication des requêtes dure 30 jours à compter de la création initiale de l’enregistrement. Une nouvelle tentative ne prolonge pas cette période. Après expiration, la même clé de requête peut lancer une nouvelle opération, sous réserve des permissions en vigueur et de vérifications indépendantes de l’identité du paiement ou de l’événement. Les événements d’utilisation acceptés conservent leurs identités durables afin d’éviter des effets comptables en double.

Les services consommateurs doivent supprimer les enregistrements de transmission de l’utilisation et de rapprochement 30 jours après leur création initiale, même si l’exécution ou l’utilisation reste inconnue ou si un rapport n’est pas parvenu à Delcore. Réessayez la transmission et la recherche de l’utilisation inconnue jusqu’à cette échéance, puis abandonnez la facturation en attente. N’estimez pas l’utilisation à des fins de facturation. Cela peut entraîner la perte définitive d’utilisation facturable, et une clé de déduplication d’exécution expirée peut permettre une nouvelle exécution et une nouvelle facturation. Les enregistrements d’audit de la plateforme et les journaux opérationnels expirent également 30 jours après leur création. Ces limites ne suppriment pas les écritures comptables déjà acceptées par Delcore et ne modifient pas la validité des clés API.

## Vérifier une clé API

Delcore émet des clés API réutilisables, personnelles et de service de tenant, pour tout service enregistré. Les utilisateurs ainsi que les propriétaires et administrateurs de tenants les créent avec un jeton de connexion Delino ; consultez [Clés API DeliTools](/fr/services/delitools/api-keys) pour le parcours côté utilisateur. Une clé commence par `dlk_`, accorde des capacités sélectionnées de services sélectionnés et appartient à un seul tenant.

Un service enregistré vérifie une clé reçue de ses appelants avec `delcore.v1.DelcoreService/AuthorizeApiKey`. Authentifiez-vous avec le jeton d’accès machine-to-machine Logto de votre service. Le service doit être enregistré et disposer de la portée `authorize`.

- Envoyez le `secret` complet, la `capability` utilisée et `paid` pour les tâches payantes. Vous pouvez également envoyer un `feature_key` et des `group_values` comme dans `Authorize`.
- Delcore n’évalue que les autorisations qui désignent le service appelant ; votre service ne peut donc jamais autoriser les capacités d’un autre service.
- Un secret mal formé, inconnu, révoqué ou expiré, ou le secret d’un tenant supprimé, renvoie une erreur de non-authentification sans aucun détail sur la clé. Delcore répond également de cette manière lorsque le jeton de votre propre service est rejeté ; vérifiez donc les informations d’authentification de votre service si toutes les clés valides échouent. Traitez cette réponse comme une information d’authentification invalide.
- Pour un secret valide, la réponse contient `allowed`, `reasons` et le champ facultatif `feature`, ainsi que `keyId`, `tenantId`, `ownerKind` et `userId`. Une clé dépourvue de la capacité renvoie `allowed: false` avec `BLOCK_REASON_PERMISSION`.
- Une clé personnelle nécessite également que son utilisateur dispose actuellement de la permission correspondant à la capacité dans le tenant. Une clé de tenant ne dépend pas de l’utilisateur qui l’a créée. Les deux nécessitent un tenant actif et les mêmes vérifications de facturation que `Authorize` lorsque `paid` est défini.
- Comparez le tenant fourni par votre appelant, le cas échéant, au `tenantId` renvoyé et rejetez toute discordance.
- Vérifiez chaque requête. Ne mettez pas de résultat en cache. Traitez les erreurs, les dépassements de délai et l’indisponibilité de Delcore comme un refus d’accès pour les nouvelles tâches payantes. Un résultat d’autorisation ne réserve rien.
