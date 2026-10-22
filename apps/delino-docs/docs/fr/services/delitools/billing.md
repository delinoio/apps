# Facturation de DeliTools

DeliTools facture à votre tenant les tokens d’IA utilisés par vos requêtes. Delcore calcule le coût de l’utilisation et gère votre solde, vos crédits et vos factures. Cette page explique comment l’utilisation donne lieu à une facturation.

## Ce qui est mesuré

- DeliTools lit les totaux de **tokens d’entrée** et de **tokens de sortie** que le fournisseur d’IA indique pour chaque tentative. Il utilise ces totaux tels qu’ils sont indiqués.
- Les totaux indiqués comprennent déjà les éventuels tokens mis en cache ou de raisonnement. DeliTools n’ajoute pas à nouveau ces éléments, n’accorde aucune réduction distincte pour le cache et n’applique aucun supplément pour le raisonnement.
- DeliTools transmet séparément à Delcore l’utilisation en entrée et en sortie, et Delcore en calcule le coût selon leurs tarifs respectifs.

## Tarifs

- L’opérateur du service définit dans Delcore un tarif par sens de circulation des tokens (entrée et sortie) pour chaque modèle. Les tarifs peuvent varier d’un modèle à l’autre.
- La traduction et l’extraction utilisent les mêmes tarifs. L’extraction utilise toujours `openai/gpt-6-luna` ; son coût est donc calculé selon les tarifs d’entrée et de sortie de ce modèle.
- Un modèle dont les deux tarifs ne sont pas enregistrés ne peut pas être exécuté. La requête échoue avec `ERROR_REASON_MODEL_UNAVAILABLE` et aucun autre modèle n’est utilisé à sa place.
- Un changement de tarif s’applique aux exécutions qui commencent après ce changement. Une utilisation déjà commencée conserve le tarif en vigueur à son démarrage.
- Votre forfait détermine l’utilisation incluse, votre solde et la possibilité de continuer une fois le solde épuisé. Consultez votre forfait et vos factures dans Delcore.

## Ce qui est facturé

- **Toute tentative dont l’utilisation est connue est facturée.** Cela inclut une tentative dont le résultat est invalide, dont l’extraction échoue, dont la réponse est arrivée avant votre annulation ou dont le résultat ne vous parvient jamais.
- **Les nouvelles tentatives sont des tentatives distinctes.** Une défaillance transitoire du fournisseur (une erreur réseau ou une réponse HTTP 5xx ou 429) ne renvoie aucune donnée d’utilisation ; DeliTools réessaie donc sans facturer la tentative échouée. Une réponse complète du fournisseur dont l’utilisation est connue met fin à la boucle de tentatives et est facturée, même si sa sortie est invalide, car DeliTools ne réessaie jamais pour corriger une sortie invalide. Une réponse sans données d’utilisation ne coûte rien.
- **Une utilisation inconnue n’est pas facturée.** Si le fournisseur ne communique pas de totaux de tokens exploitables, DeliTools n’estime aucun coût et ne recherche pas ces données d’utilisation ultérieurement.
- **Les appels interrompus ne sont pas facturés.** Si vous annulez, ou si le délai d’exécution expire, alors que l’appel au fournisseur est toujours en cours, aucun rapport d’utilisation ne parvient et rien n’est facturé, même si le fournisseur avait déjà commencé le travail. Il s’agit d’une limite connue : DeliTools ne recherche pas ces données d’utilisation après coup.
- **L’utilisation doit être enregistrée.** DeliTools enregistre l’utilisation de chaque tentative avant de répondre. S’il ne peut pas l’enregistrer, la requête échoue avec `ERROR_REASON_DEPENDENCY_UNAVAILABLE` et vous ne recevez aucun résultat.
- **Les requêtes rejetées ne sont pas facturées.** Les requêtes qui échouent à la validation, à l’authentification ou à l’autorisation, ou qui se heurtent à une limite ou à un schéma non pris en charge, ne parviennent jamais au fournisseur d’IA.
- **Chaque appel est nouveau.** DeliTools ne dispose ni de clé d’idempotence ni de mécanisme de renvoi d’un résultat précédent. Si vous envoyez à nouveau la même requête, elle est exécutée et facturée à nouveau.

## Quand les frais apparaissent

DeliTools enregistre l’utilisation de chaque tentative avant de vous répondre, puis la transmet à Delcore. La transmission des données d’utilisation est indépendante de votre requête ; des frais peuvent donc apparaître dans Delcore peu après l’appel. Si Delcore est indisponible, DeliTools continue de réessayer avec les mêmes identifiants, de sorte que les nouvelles tentatives ne créent pas de frais en double.

Les enregistrements d’utilisation que DeliTools ne parvient pas à transmettre sont supprimés 30 jours après leur création, et les nouvelles tentatives ne prolongent pas ce délai. L’utilisation qui n’a jamais été transmise pendant ce délai n’est pas facturée. DeliTools ne l’estime pas.

Vous pouvez consulter votre utilisation et vos frais dans Delcore. Les réponses de DeliTools ne les incluent pas.

## Vérification du solde et dépassement

Delcore vérifie votre autorisation et votre statut de facturation avant chaque requête, puis à nouveau avant chaque nouvelle tentative. Il ne réserve ni argent ni tokens pour l’appel. Le coût n’est connu qu’une fois l’appel terminé. Plusieurs requêtes exécutées simultanément peuvent donc consommer davantage que votre solde restant ou votre utilisation incluse. Lorsque le solde ou une limite est épuisé, Delcore bloque les requêtes suivantes avec `ERROR_REASON_BILLING_BLOCKED`.

Si Delcore est injoignable, DeliTools refuse les nouvelles requêtes payantes au lieu de les exécuter.

## Requêtes bloquées

`ERROR_REASON_BILLING_BLOCKED` signifie que Delcore a bloqué l’utilisation payante pour votre tenant. Les causes courantes sont un solde épuisé, un échec de renouvellement ou une période payante expirée, un litige en cours, un blocage par l’opérateur ou un tenant en cours de suppression. Résolvez la cause dans Delcore. Par exemple, rechargez votre solde ou renouvelez votre abonnement. La gestion du tenant et la régularisation des paiements restent disponibles lorsque l’utilisation payante est bloquée.
