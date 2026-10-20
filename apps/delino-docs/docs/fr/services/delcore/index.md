# Delcore

Delcore fournit des tenants partagés, un contrôle d’accès et une facturation pour les services Delino. Le serveur d’API officiel est `https://delcore.api.delino.io`. Cette page décrit la vérification de l’état d’un tenant destinée aux développeurs de services.

## Vérifier l’état d’un tenant

Un service enregistré peut demander à Delcore si un tenant est actif, bloqué ou supprimé. Utilisez cette vérification lorsque votre service stocke des données pour un tenant, mais ne dispose pas d’un utilisateur Delino pour chaque requête, par exemple lorsqu’une clé d’API de projet effectue l’appel.

Appelez `delcore.v1.DelcoreService/GetTenantStates` avec 1 à 100 identifiants de tenant. Chaque identifiant doit être un UUID version 7 en minuscules. Authentifiez-vous avec le jeton d’accès de l’application machine à machine Logto de votre service. Le service doit être enregistré auprès de Delcore et disposer de la portée `authorize`. Les jetons utilisateur, les services non enregistrés et les services ne disposant pas de cette portée ne peuvent pas lire l’état des tenants.

La réponse reprend chaque identifiant de tenant demandé, dans l’ordre où vous les avez envoyés, avec l’un des états suivants :

| État                    | Signification                                                   |
| ----------------------- | --------------------------------------------------------------- |
| `TENANT_STATE_ACTIVE`   | Le tenant existe et n’est ni bloqué ni en cours de suppression. |
| `TENANT_STATE_BLOCKED`  | Un opérateur de la plateforme a bloqué le tenant.               |
| `TENANT_STATE_DELETING` | La suppression est en attente.                                  |
| `TENANT_STATE_DELETED`  | La suppression est définitive.                                  |
| `TENANT_STATE_UNKNOWN`  | Delcore ne connaît aucun tenant avec cet identifiant.           |

Lorsque plusieurs conditions s’appliquent, `DELETED` prévaut sur `DELETING`, `DELETING` prévaut sur `BLOCKED` et `BLOCKED` prévaut sur `ACTIVE`.

L’état ne dépend pas de la facturation. Un abonnement expiré, un échec de renouvellement, un litige ou un solde faible ne font pas passer un tenant à l’état `BLOCKED`. Pour déterminer si un utilisateur peut exécuter un travail payant, utilisez plutôt `Authorize`. Cette vérification se limite à lire des données. Elle ne modifie pas un tenant, n’autorise pas d’utilisation payante, ne contacte aucun prestataire de paiement et ne renvoie aucun nom de tenant, détail sur les utilisateurs ou renseignement de facturation.

## Gérer les résultats et les échecs

- Autorisez l’accès uniquement pour `TENANT_STATE_ACTIVE`, ou lorsque vos propres règles documentées permettent aux propriétaires ou aux administrateurs d’accéder à un tenant bloqué ou en cours de suppression.
- Considérez `TENANT_STATE_UNKNOWN`, les erreurs de requête, les expirations de délai et l’indisponibilité de Delcore comme une absence d’accès. Ne supprimez pas de données dans ces cas.
- Ne supprimez automatiquement les données de tenant que vous conservez qu’après avoir reçu explicitement `TENANT_STATE_DELETED`. `BLOCKED` et `DELETING` ne permettent jamais une suppression automatique.
- Les identifiants non valides, une requête vide ou plus de 100 identifiants renvoient une erreur d’argument non valide. Des identifiants d’authentification manquants, expirés ou non valides renvoient une erreur de non-authentification. Un appelant qui n’est pas autorisé à lire l’état des tenants reçoit une erreur d’autorisation refusée. Les autres échecs renvoient une erreur accompagnée d’un identifiant de corrélation ; lorsque vous demandez de l’assistance, incluez cet identifiant, qui figure dans les détails de l’erreur et dans l’en-tête `X-Delcore-Correlation-Id`.
- Réessayez ultérieurement une requête ayant échoué. L’échec d’une requête ne signifie jamais qu’un tenant est actif ou supprimé.
