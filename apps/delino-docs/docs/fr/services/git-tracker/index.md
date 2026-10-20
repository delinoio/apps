# Git Tracker

Suivez des mesures numériques pour les commits Git sur [git-tracker.delino.io](https://git-tracker.delino.io). Git Tracker est gratuit. Il prend en charge les dépôts GitHub publics et privés, y compris les pull requests provenant de forks. Utilisez-le pour les tailles de fichiers, les durées, les débits, les nombres et d’autres mesures scalaires.

## Connecter un dépôt

1. Connectez-vous avec votre compte Delino et sélectionnez votre espace personnel ou celui de votre organisation.
2. Demandez à un propriétaire de l’espace de vous accorder l’autorisation Afficher, Modifier ou Gérer de Git Tracker par l’intermédiaire des groupes de l’espace. Afficher permet la lecture. Modifier ajoute les téléversements et la finalisation. Gérer ajoute la connexion, la gestion des jetons et la suppression définitive.
3. Installez l’application GitHub Git Tracker pour le dépôt.
4. Ouvrez **Gérer les dépôts**. Saisissez `owner/repository` et un jeton d’accès GitHub pour votre compte GitHub associé, disposant d’un accès administrateur au dépôt. Le service vérifie votre autorité GitHub et ne conserve pas ce jeton d’accès.
5. Connectez le dépôt. Vous devez également disposer de l’autorité Propriétaire/Administrateur dans l’espace Delino. Un dépôt ne peut être connecté qu’à un seul espace à la fois. Copiez son identifiant de connexion depuis la gestion des dépôts pour les appels à l’Action et à l’API. Après la déconnexion, un autre espace autorisé peut le connecter sous un nouvel identifiant de connexion ; l’historique conservé par l’ancien espace reste privé.

Les mesures des dépôts publics sont lisibles sans connexion. Les mesures privées nécessitent d’être membre de l’espace et de disposer de l’autorisation de lecture de Git Tracker. L’accès à l’espace s’applique à tous les dépôts connectés dans cet espace.

La déconnexion ou la suppression de l’application arrête les téléversements et rend privé l’historique conservé. Les membres autorisés peuvent toujours le consulter. Un passage confirmé de public à privé rend privées les données historiques. Si GitHub ne peut pas confirmer la visibilité actuelle, Git Tracker conserve la dernière visibilité confirmée jusqu’à ce que la synchronisation réussisse.

## Définir votre jeu de données

Conservez une configuration YAML dans votre dépôt. Les identifiants sont stables ; les noms peuvent changer sans perte d’historique. Toutes les colonnes d’une ligne doivent être téléversées ensemble.

```yaml
id: file-size
name: JavaScript file sizes
columns:
  - id: original
    name: Original size
    unit: B
    direction: lower
    absoluteThreshold: 1024
  - id: minified
    name: Minified size
    unit: B
    direction: lower
    percentageThreshold: 1
  - id: gzip
    name: Gzipped size
    unit: B
    direction: lower
    absoluteThreshold: 128
    percentageThreshold: 2
```

Téléversez des lignes JSON, par exemple un tableau de tailles de fichiers de type SWC :

```json
{
  "rows": [
    { "id": "jquery", "name": "jQuery", "values": { "original": 285314, "minified": 87533, "gzip": 30321 } },
    { "id": "three", "name": "Three.js", "values": { "original": 1321000, "minified": 675000, "gzip": 170000 } }
  ]
}
```

L’Action préserve l’écriture décimale JSON sans conversion en nombre à virgule flottante binaire. Les appelants de Direct Connect envoient les valeurs décimales sous forme de chaînes, comme `"9007199254740993.0000000000000001"`. Les nombres négatifs et les décimaux finis sont pris en charge. L’arrondi d’affichage ne modifie jamais les décisions de régression.

Les unités prises en charge sont `B`, `kB`, `MB`, `GB`, `KiB`, `MiB`, `GiB` ; `ns`, `μs`, `ms`, `s` ; `count`, `%`, `unitless` ; et `ops/s`, `B/s`, `kB/s`, `MB/s`, `GB/s`, `KiB/s`, `MiB/s`, `GiB/s`. Les unités compatibles sont converties avant la comparaison. Les types incompatibles ne sont pas comparables. Par exemple, 1048576 B s’affiche comme 1 MiB ; les valeurs exactes restent visibles dans les tableaux.

Les directions sont `lower`, `higher` et `neutral`. Les colonnes neutres ne produisent jamais de régressions. Les seuils s’appliquent à toutes les lignes de la colonne. Lorsque des seuils absolu et en pourcentage sont tous deux définis, le dépassement de l’un ou de l’autre constitue une régression ; l’égalité n’en est pas une. Sans seuil, les changements sont affichés, mais ne provoquent pas d’échec pour régression. Seuls les changements dans le sens de la dégradation sont évalués.

La variation en pourcentage est `(new - baseline) / abs(baseline) * 100`. Une référence égale à zéro n’a pas de variation en pourcentage. Un seuil absolu peut tout de même permettre de trancher ; une règle fondée uniquement sur un pourcentage reporte le jugement. Les comparaisons utilisent la configuration enregistrée de l’exécution cible, et non une configuration ultérieure.

## Utiliser l’Action GitHub officielle

Épinglez `delinoio/apps/actions/git-tracker` à un SHA de commit complet et vérifié. Remplacez les espaces réservés ci-dessous par votre identifiant de connexion Git Tracker, la révision de checkout et la révision de l’Action. Accordez `id-token: write` pour OIDC. Un jeton de téléversement du dépôt constitue une autre possibilité ; stockez-le comme secret GitHub et transmettez `token`.

```yaml
permissions:
  contents: read
  id-token: write
jobs:
  measure:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@<reviewed-checkout-commit-sha>
      - run: ./measure.sh
      - uses: delinoio/apps/actions/git-tracker@<reviewed-action-commit-sha>
        with:
          repository-id: '019b0000-0000-7000-8000-000000000001'
          configuration: measurements.yaml
          measurements: measurements.json
          fail-on-regression: 'false'
  mark-as-done:
    needs: measure
    if: always()
    runs-on: ubuntu-latest
    steps:
      - uses: delinoio/apps/actions/git-tracker@<reviewed-action-commit-sha>
        with:
          operation: complete
          repository-id: '019b0000-0000-7000-8000-000000000001'
```

Les tâches indépendantes partagent l’identifiant d’exécution par défaut, `repository_id/run_id/run_attempt`, et peuvent téléverser des lignes distinctes. Utilisez un nouvel identifiant d’exécution pour les réexécutions. L’OIDC des Actions exige l’identité par défaut ; les identifiants d’exécution personnalisés sont destinés aux appelants utilisant un jeton de dépôt. Les téléversements OIDC provenant de forks sont limités à la tête de la PR et ne peuvent pas fournir de remplacements de référence ni écrire dans l’historique des pushs en amont.

Chaque tâche de téléversement évalue uniquement les lignes qu’elle a téléversées. Par défaut, les régressions sont signalées sans faire échouer la CI. Définissez `fail-on-regression: 'true'` pour faire échouer cette tâche en cas de régression. Les comparaisons manquantes ne font pas échouer la CI. La tâche de finalisation indique uniquement que la mesure est terminée ; elle ne transmet pas l’état de réussite, d’échec ou d’annulation de la CI et n’effectue aucun contrôle global. Une finalisation vide est valide lorsque toutes les tâches ont été ignorées.

Les erreurs réseau et les défaillances transitoires de l’API font l’objet d’au plus trois tentatives. Par défaut, la tâche échoue après ces tentatives. Définissez `warn-on-api-error: 'true'` pour émettre un avertissement et laisser la tâche réussir à la place. Les échecs de commentaires de PR émettent uniquement un avertissement ; ils n’invalident pas les données enregistrées et ne font pas échouer la CI de manière indépendante. L’Action journalise un lien vers les détails et écrit un récapitulatif de tâche.

Un seul commentaire de PR reflète les résultats cumulés de la dernière exécution enregistrée pour le dernier commit de la PR. Les exécutions plus anciennes restent disponibles dans l’historique web.

## Appeler l’API Connect

L’origine de l’API est `https://git-tracker.api.delino.io`. Les requêtes JSON Connect utilisent `Content-Type: application/json` et `Connect-Protocol-Version: 1`. Les gestionnaires peuvent émettre des jetons de dépôt avec une date d’expiration future obligatoire et les révoquer depuis la gestion des dépôts.

```sh
curl --fail-with-body \
  'https://git-tracker.api.delino.io/gittracker.v1.GitTrackerService/Upload' \
  -H "Authorization: Bearer ${GIT_TRACKER_TOKEN}" \
  -H 'Content-Type: application/json' \
  -H 'Connect-Protocol-Version: 1' \
  --data-binary @upload.json
```

`upload.json` contient `context`, `dataset` et `rows` :

```json
{
  "context": {
    "repositoryId": "019b0000-0000-7000-8000-000000000001",
    "executionId": "my-historical-run",
    "commit": "0123456789012345678901234567890123456789",
    "branch": "main"
  },
  "dataset": {
    "id": "size",
    "name": "Size",
    "columns": [{ "id": "bytes", "name": "Bytes", "unit": "B", "direction": "DIRECTION_LOWER", "absoluteThreshold": "1024" }]
  },
  "rows": [{ "id": "bundle", "name": "Bundle", "values": { "bytes": "1048576" } }]
}
```

Appelez `/gittracker.v1.GitTrackerService/Complete` avec le même `context`. Pour les téléversements de PR, incluez `pullRequest` et utilisez la branche de base ainsi que le commit de tête de la PR. `baselineCommit` sélectionne un SHA complet explicite comme référence. Les commits historiques utilisent la même API de téléversement. Ne convertissez pas les chaînes décimales avec `Number` de JavaScript avant de les envoyer.

## Règles d’exécution et de comparaison

Le premier téléversement fige la configuration de chaque jeu de données pour cette exécution. Toute configuration ultérieure différente est rejetée. La répétition d’une ligne existante avec des valeurs numériques identiques est idempotente. Les valeurs conflictuelles sont rejetées. Un lot est atomique : si une ligne est invalide ou conflictuelle, aucune donnée de cette requête n’est enregistrée.

Les données apparaissent immédiatement avec l’état **Mesure en cours**. Elles ne deviennent représentatives qu’après la finalisation. Le résultat représentatif d’un commit correspond à sa dernière exécution enregistrée et finalisée, y compris les exécutions comportant des régressions. Une exécution plus ancienne qui se termine tardivement ne remplace pas une exécution enregistrée et finalisée plus récente. Une finalisation répétée ne modifie ni l’heure de finalisation ni l’ordre.

Après six heures sans nouvelle donnée de mesure enregistrée, une exécution non finalisée passe à l’état **Interrompue**. Les retransmissions identiques ne réinitialisent pas ce délai. Les exécutions finalisées et interrompues rejettent tout téléversement supplémentaire ; les exécutions interrompues rejettent également la finalisation. Démarrez plutôt une nouvelle exécution. Les données restent disponibles jusqu’à leur suppression.

Les comparaisons automatiques de PR figent le commit de base de fusion et son exécution représentative finalisée lors du premier téléversement. Une référence manquante reste indisponible même si elle est téléversée ultérieurement. Les comparaisons automatiques de push parcourent séparément, pour chaque ligne, l’ascendance du premier parent et utilisent l’ancêtre mesuré le plus proche qui a été finalisé avant l’instant limite du premier téléversement. Les tâches ultérieures utilisent ce même instant limite. Le tableau indique chaque commit et chaque exécution effectivement utilisés comme source. Les comparaisons avec un SHA de référence explicite n’utilisent pas de repli vers un ancêtre au niveau des lignes.

La comparaison entre deux commits du site web utilise uniquement les exécutions représentatives finalisées des commits sélectionnés. Les nouvelles lignes ou colonnes ainsi que les données manquantes ou incompatibles ne sont pas comparables et ne provoquent pas d’échec pour régression. Les données de référence supprimées ne sont pas reconstituées. Avant la finalisation, une ligne absente n’a pas encore été collectée ; après la finalisation, elle est manquante.

Les graphiques utilisent des commits espacés uniformément dans l’ordre du premier parent. Ils relient les points effectivement mesurés à travers les commits ignorés et ne reportent ni n’inventent jamais de valeurs. Filtrez par jeu de données, branche, colonne, éléments, dates et plage de commits. Utilisez le tableau de données pour consulter les valeurs exactes et les liens d’exécution accessibles au clavier.

## Suppression et dépannage

Après confirmation, les gestionnaires peuvent supprimer définitivement les mesures d’un dépôt, un jeu de données ou une exécution. Une comparaison dont la référence a été supprimée devient indisponible. Lorsque Delcore finalise la suppression d’un espace, Git Tracker supprime les connexions, les jetons, la configuration et l’historique de cet espace.

En cas d’erreur d’autorisation, vérifiez l’espace sélectionné et votre rôle Git Tracker actuel. En cas de conflits de téléversement, vérifiez l’identité de l’exécution, la configuration du jeu de données et les valeurs complètes des lignes. Pour les exécutions interrompues ou finalisées, utilisez un nouvel identifiant d’exécution. En cas d’erreurs réseau ou serveur, réessayez et conservez l’identifiant de corrélation de la réponse lorsque vous demandez de l’aide. Ne partagez jamais de jetons, d’identifiants de connexion ou de mesures privées dans des rapports publics.

Les mesures d’exécution des PR restent consultables, mais ne fournissent ni points aux graphiques des branches en amont ni références automatiques. Cela empêche les mesures provenant de forks de remplacer les données ou la configuration de la branche de référence pour un SHA de commit partagé.
