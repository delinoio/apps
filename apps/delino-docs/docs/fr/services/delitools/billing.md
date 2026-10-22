# Facturation de DeliTools

DeliTools facture à votre tenant les jetons d’IA utilisés par vos requêtes. Delcore calcule le coût de l’utilisation et gère votre solde, vos crédits et vos factures. Cette page explique comment l’utilisation se traduit en frais.

## Ce qui est mesuré

- DeliTools lit les totaux de **jetons d’entrée** et de **jetons de sortie** signalés par le fournisseur d’IA pour chaque tentative. Il utilise ces totaux tels qu’ils sont signalés.
- Les totaux signalés incluent déjà les éventuels jetons mis en cache ou de raisonnement. DeliTools ne les ajoute pas à nouveau, n’accorde aucune remise distincte pour le cache et n’applique aucun supplément pour le raisonnement.
- DeliTools transmet séparément l’utilisation en entrée et en sortie à Delcore, qui leur applique leurs tarifs respectifs.

## Tarifs

- L’opérateur du service définit dans Delcore un tarif par sens des jetons (entrée et sortie) pour chaque modèle. Les tarifs peuvent varier d’un modèle à l’autre.
- La traduction et l’extraction utilisent les mêmes tarifs. L’extraction utilise toujours `openai/gpt-6-luna` ; son coût est donc calculé selon les tarifs d’entrée et de sortie de ce modèle.
- Un modèle pour lequel les deux tarifs ne sont pas enregistrés ne peut pas être exécuté. La requête échoue avec `ERROR_REASON_MODEL_UNAVAILABLE` et aucun autre modèle n’est utilisé à sa place.
- Une modification de tarif s’applique aux exécutions qui démarrent après cette modification. L’utilisation déjà commencée conserve le tarif en vigueur à son démarrage.
- Votre forfait détermine l’utilisation incluse, votre solde et la possibilité de continuer une fois le solde épuisé. Consultez votre forfait et vos factures dans Delcore.

## Ce qui est facturé

- **Chaque tentative dont l’utilisation est connue est facturée.** Cela inclut une tentative dont le résultat est invalide, dont l’extraction échoue, dont la réponse est arrivée avant votre annulation ou dont le résultat ne vous parvient jamais.
- **Les nouvelles tentatives sont des tentatives distinctes.** Si DeliTools réessaie après une défaillance temporaire du fournisseur et que la tentative échouée a signalé une utilisation, les deux tentatives sont facturées. Une tentative échouée qui n’a signalé aucune utilisation ne coûte rien.
- **L’utilisation inconnue n’est pas facturée.** Si le fournisseur ne signale pas de totaux de jetons exploitables, DeliTools n’estime pas de frais et ne recherche pas cette utilisation ultérieurement.
- **Les appels interrompus ne sont pas facturés.** Si vous annulez, ou si le délai d’exécution expire, alors que l’appel au fournisseur est encore en cours, aucun rapport d’utilisation n’arrive et rien n’est facturé, même si le fournisseur avait déjà commencé le travail. Il s’agit d’une limitation connue : DeliTools ne recherche pas cette utilisation après coup.
- **L’utilisation doit être enregistrée.** DeliTools enregistre l’utilisation de chaque tentative avant de répondre. S’il ne peut pas l’enregistrer, la requête échoue avec `ERROR_REASON_DEPENDENCY_UNAVAILABLE` et vous ne recevez aucun résultat.
- **Les requêtes rejetées ne sont pas facturées.** Les requêtes qui échouent à la validation, à l’authentification ou à l’autorisation, ou qui atteignent une limite ou utilisent un schéma non pris en charge, ne parviennent jamais au fournisseur d’IA.
- **Chaque appel est nouveau.** DeliTools ne dispose ni de clé d’idempotence ni de mécanisme de renvoi d’un résultat précédent. Si vous envoyez à nouveau la même requête, elle est exécutée à nouveau et facturée à nouveau.

## Quand les frais apparaissent

DeliTools enregistre l’utilisation de chaque tentative avant de vous répondre, puis la transmet à Delcore. La transmission de l’utilisation est indépendante de votre requête ; des frais peuvent donc apparaître dans Delcore peu après l’appel. Si Delcore est indisponible, DeliTools continue de réessayer avec les mêmes identifiants, de sorte que les nouvelles tentatives ne créent pas de frais en double.

Les enregistrements d’utilisation que DeliTools ne parvient pas à transmettre sont supprimés 30 jours après leur création, et les nouvelles tentatives ne prolongent pas cette période. L’utilisation qui n’a jamais été transmise pendant ce délai n’est pas facturée. DeliTools ne l’estime pas.

Vous pouvez consulter votre utilisation et vos frais dans Delcore. Les réponses de DeliTools ne les incluent pas.

## Vérification du solde et dépassement

Delcore vérifie votre autorisation et votre statut de facturation avant chaque requête, puis à nouveau avant chaque nouvelle tentative. Il ne réserve ni argent ni jetons pour l’appel. Le coût n’est connu qu’une fois l’appel terminé. Plusieurs requêtes exécutées simultanément peuvent donc dépasser votre solde restant ou votre utilisation incluse. Lorsque le solde ou une limite est épuisé, Delcore bloque les requêtes suivantes avec `ERROR_REASON_BILLING_BLOCKED`.

Si Delcore est injoignable, DeliTools refuse les nouvelles requêtes payantes au lieu de les exécuter.

## Requêtes bloquées

`ERROR_REASON_BILLING_BLOCKED` signifie que Delcore a bloqué l’utilisation payante pour votre tenant. Les causes courantes sont un solde épuisé, un échec de renouvellement ou une période payante expirée, un litige en cours, un blocage par l’opérateur ou un tenant en cours de suppression. Résolvez la cause dans Delcore. Par exemple, rechargez votre solde ou renouvelez votre abonnement. La gestion du tenant et la régularisation des paiements restent disponibles pendant le blocage de l’utilisation payante.
