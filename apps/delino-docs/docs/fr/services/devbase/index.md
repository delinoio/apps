# DevBase

DevBase est un annuaire gratuit des utilisateurs finaux de vos applications. Vous enregistrez vos propres utilisateurs, les recherchez et les modifiez au moyen d’une API Connect. Vos utilisateurs n’ont pas besoin de comptes Delino, et votre application conserve son propre système de connexion. DevBase attribue à chaque utilisateur un identifiant stable que vous pouvez transmettre à d’autres services.

## Concepts

- **Tenant :** un espace personnel ou une organisation Delino. Delcore gère les tenants et leurs membres.
- **Projet :** un annuaire nommé d’utilisateurs finaux au sein d’un tenant. Les projets ne partagent pas leurs utilisateurs, même au sein du même tenant.
- **Utilisateur final :** une personne ou un compte dans votre application. Il possède un identifiant externe que vous choisissez, un identifiant interne créé par DevBase, un nom et une adresse e-mail facultatifs, ainsi qu’un objet JSON de métadonnées.
- **Clé d’API de projet :** un secret que votre application utilise pour lire ou modifier les utilisateurs finaux d’un projet.

## Connexion et autorisations

Envoyez un en-tête `Authorization: Bearer <token>` avec chaque requête.

- **Les développeurs** utilisent leur jeton de connexion Delino. Seuls les Owner ou Admin actuels du tenant peuvent gérer les projets et les clés d’API. Ils peuvent également lire et modifier les utilisateurs finaux avec le jeton de connexion.
- **Les applications** utilisent une clé d’API de projet. Une clé commence par `dbk_`. Une clé `READ_ONLY` recherche et répertorie les utilisateurs. Une clé `CRUD` peut également créer, mettre à jour et supprimer des utilisateurs. Une clé ne permet jamais de gérer les projets ou les clés, et elle fonctionne uniquement pour son propre projet.

Les utilisateurs finaux ne se connectent jamais à DevBase. Leurs enregistrements ne leur donnent pas accès à Delino.

## Démarrage rapide

1. **Créez un projet.** Appelez `CreateProject` avec l’identifiant de votre tenant et un nom. Les noms sont uniques au sein d’un tenant, sans tenir compte de la casse.
2. **Émettez une clé.** Appelez `IssueApiKey` avec l’identifiant du projet et une autorisation. La réponse contient le secret une seule fois. Stockez-le dans votre gestionnaire de secrets.
3. **Enregistrez les utilisateurs.** Appelez `CreateEndUser` avec la clé. Stockez l’`id` renvoyé avec votre propre enregistrement utilisateur.
4. **Recherchez les utilisateurs.** Utilisez `GetEndUser` avec l’identifiant interne ou `LookupEndUser` avec l’identifiant externe.
5. **Modifiez ou supprimez les utilisateurs.** Utilisez `UpdateEndUser` et `DeleteEndUser` avec la `version` actuelle.

Toutes les procédures se trouvent sur `devbase.v1.DevBaseService`. Appelez DevBase depuis du code côté serveur ou la ligne de commande, avec n’importe quel client Connect ou avec du JSON brut sur HTTP. Ne l’appelez pas depuis un navigateur et ne placez pas de clé de projet dans du code frontend : DevBase rejette toutes les origines de navigateur à l’exception de sa propre console. Définissez `DEVBASE_URL` sur l’URL de base de votre serveur d’API DevBase, par exemple `http://127.0.0.1:46317` pour un serveur local.

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

Dans JSON, les noms de champs utilisent lowerCamelCase, les valeurs d’énumération utilisent leur nom complet et les nombres 64 bits tels que `version` sont des chaînes de caractères.

## Projets

- Le nom d’un projet comporte de 1 à 256 octets, sans espace au début ni à la fin.
- `UpdateProject` modifie le nom. `DeleteProject` supprime en une seule opération le projet, tous ses utilisateurs et toutes ses clés.
- Un tenant peut comporter jusqu’à 100 projets.

## Clés d’API de projet

- Un projet peut comporter jusqu’à 20 clés actives. Une clé reste active jusqu’à sa révocation ou son expiration.
- Définissez `expire_time` lors de l’émission d’une clé si elle doit expirer. Sans cette valeur, la clé n’expire pas automatiquement.
- DevBase stocke uniquement une empreinte du secret. Si vous perdez un secret, révoquez cette clé et émettez-en une nouvelle. Pour effectuer la rotation d’une clé, émettez d’abord la nouvelle clé, puis révoquez l’ancienne.
- Si la réponse à `IssueApiKey` est perdue, répétez la requête avec la même clé d’idempotence. La réponse répétée contient l’identifiant et l’autorisation de la clé, mais pas le secret.
- `RevokeApiKey` prend effet immédiatement. Une clé révoquée ou expirée renvoie `unauthenticated`.

## Utilisateurs finaux

| Champ         | Règle                                                                                                          |
| ------------- | -------------------------------------------------------------------------------------------------------------- |
| `external_id` | Obligatoire. De 1 à 256 octets. Sensible à la casse. Unique dans le projet. Aucun espace au début ou à la fin. |
| `name`        | Facultatif. Jusqu’à 256 octets.                                                                                |
| `email`       | Facultatif. Une adresse simple de 254 octets maximum. Plusieurs utilisateurs peuvent avoir la même adresse.    |
| `metadata`    | Un objet JSON de 16 Kio maximum. Une requête complète peut atteindre 64 Kio.                                   |

- Le même identifiant externe peut exister dans différents projets. Un projet peut comporter jusqu’à 1 000 000 d’utilisateurs.
- La création est explicite. DevBase ne propose pas d’upsert ni de recherche par adresse e-mail, nom ou métadonnées.
- `UpdateEndUser` utilise `update_mask` avec `external_id`, `name`, `email` et `metadata`. Un champ `name` ou `email` inclus dans le masque avec une valeur vide est effacé. Un champ `metadata` inclus dans le masque remplace l’objet entier, et un objet vide l’efface. Vous ne pouvez pas effacer l’identifiant externe. Vous pouvez le modifier, et l’identifiant interne reste inchangé.
- `DeleteEndUser` supprime l’utilisateur immédiatement et définitivement. Vous pouvez réutiliser immédiatement l’identifiant externe. L’utilisateur suivant portant cet identifiant reçoit un nouvel identifiant interne et n’a aucun lien avec l’utilisateur supprimé.
- DevBase n’informe pas les autres services des suppressions. Supprimez les données en aval dans votre propre application.

## Listes

`ListProjects`, `ListApiKeys` et `ListEndUsers` renvoient les éléments dans leur ordre de création. La taille de page par défaut est de 50 et la taille maximale est de 100. Répétez l’appel avec `next_page_token` jusqu’à ce qu’il soit vide. Traitez le jeton comme une valeur opaque. Les listes ne fournissent pas de nombre total. Si vous créez ou supprimez des éléments pendant la lecture des pages, une page peut omettre ou ignorer ces éléments. Un élément qui existe pendant toute la lecture apparaît une seule fois.

## Nouvelles tentatives et conflits

- Chaque modification nécessite une `idempotency_key` de 8 à 128 caractères parmi `A-Z`, `a-z`, `0-9`, `.`, `_`, `:` et `-`. Un UUID convient bien.
- Si vous répétez une requête avec la même clé et le même contenu dans un délai de 24 heures, vous obtenez le premier résultat et DevBase n’effectue pas de nouveau la modification. Utilisez ce mécanisme pour résoudre les réponses perdues. Après 24 heures, la clé correspond à une nouvelle requête.
- La même clé utilisée avec un contenu différent renvoie `already_exists`. Chaque appelant possède ses propres clés ; un autre appelant ne peut donc ni voir ni récupérer votre résultat.
- Une requête répétée est de nouveau vérifiée par rapport aux autorisations actuelles. Votre accès peut prendre fin entre le premier appel et sa répétition.
- Après la suppression d’un utilisateur, la répétition de la création ou de la mise à jour de cet utilisateur renvoie `not_found`, et DevBase ne recrée pas l’utilisateur. La répétition de la suppression réussit. Après la suppression d’un projet, les clés de ce projet cessent de fonctionner.
- `UpdateProject`, `UpdateEndUser`, `DeleteProject` et `DeleteEndUser` nécessitent la `version` actuelle. Si quelqu’un a modifié l’élément avant vous, vous obtenez `aborted`. Relisez l’élément et réessayez avec la nouvelle version.

## Limites

| Limite                          | Valeur par défaut                         |
| ------------------------------- | ----------------------------------------- |
| Projets par tenant              | 100                                       |
| Utilisateurs par projet         | 1 000 000                                 |
| Clés actives par projet         | 20                                        |
| Requêtes utilisateur par projet | 1 000 par seconde, avec des pics de 2 000 |
| Requêtes de gestion par tenant  | 50 par seconde, avec des pics de 100      |

Les requêtes utilisateur sont comptabilisées ensemble pour toutes les clés et tous les jetons de connexion d’un projet. Les requêtes de gestion sont comptabilisées ensemble pour un tenant. Une nouvelle clé n’ajoute pas de capacité. Si vous dépassez une limite, vous obtenez `resource_exhausted`. Pour une limite de débit, l’erreur et l’en-tête `Retry-After` indiquent combien de temps attendre. Un opérateur peut modifier ces limites. Elles protègent le service et ne garantissent pas une certaine vitesse.

## Erreurs

| Code                 | Signification                                                                                                |
| -------------------- | ------------------------------------------------------------------------------------------------------------ |
| `unauthenticated`    | Le jeton ou la clé est absent, invalide, expiré ou révoqué.                                                  |
| `permission_denied`  | Vous ne disposez pas du rôle ou de l’autorisation de clé requis, ou l’état du tenant bloque l’opération.     |
| `not_found`          | L’élément n’existe pas, appartient à un autre projet ou a été supprimé.                                      |
| `invalid_argument`   | Une valeur d’entrée n’est pas valide.                                                                        |
| `already_exists`     | Un nom ou un identifiant externe existe déjà, ou une clé d’idempotence a été utilisée avec un autre contenu. |
| `aborted`            | La version est ancienne.                                                                                     |
| `resource_exhausted` | Une limite de débit ou de ressources s’applique.                                                             |
| `unavailable`        | Une dépendance ou la base de données a échoué. Une modification peut avoir été appliquée ou non.             |

Chaque réponse, ainsi que chaque erreur renvoyée par DevBase lui-même, possède un identifiant de trace. Incluez-le lorsque vous demandez de l’assistance. Une requête supérieure à 256 Kio est rejetée plus tôt par la couche de transport, avec `resource_exhausted` et sans identifiant de trace ; réduisez plutôt la taille de la requête. Si vous obtenez `unavailable` pour une modification, répétez la même requête avec la même clé d’idempotence afin de déterminer si elle a été appliquée.

## État du tenant

DevBase est gratuit. Votre offre, votre solde et vos factures ne le limitent jamais. Seuls le blocage par un opérateur et la suppression du tenant peuvent le faire.

- Lorsqu’un tenant est bloqué ou en cours de suppression, les clés d’API de projet ne fonctionnent pas. Les Owner et Admin peuvent toujours lire les projets et les clés, révoquer les clés et supprimer des utilisateurs et des projets. Ils ne peuvent rien créer ni modifier, émettre des clés ou lire les utilisateurs.
- Si DevBase ne peut pas confirmer l’état du tenant, il refuse la requête avec `unavailable` ou `permission_denied` et ne fait aucune supposition.
- Après la suppression définitive d’un tenant, DevBase supprime automatiquement ses projets, ses utilisateurs et ses clés.

## Ce que DevBase ne fait pas

DevBase ne connecte pas vos utilisateurs, ne crée pas de comptes Delino pour eux et n’agit pas en leur nom. Il ne partage pas les utilisateurs entre les projets, ne déplace pas les projets entre les tenants, n’importe ni n’exporte de données et ne recherche pas les utilisateurs. Les sauvegardes éventuelles ne sont pas concernées par la suppression immédiate que DevBase applique à ses données actives.
