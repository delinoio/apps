# DeliTools

DeliTools propose deux API payantes aux développeurs : la traduction d’une liste de textes et l’extraction d’une valeur JSON à partir d’un texte. Vous les appelez avec n’importe quel client Connect, ou avec du JSON simple via HTTP, depuis votre propre code côté serveur. Delcore gère vos clés API, vos autorisations et la facturation, afin que vous utilisiez le même compte Delino et le même tenant pour tout.

## Concepts

- **Traduction :** envoyez une liste de textes, une langue cible et un modèle. Vous recevez les traductions dans le même ordre et en même nombre.
- **Extraction :** envoyez un texte et un schéma JSON Schema. Vous recevez un document JSON conforme au schéma.
- **Tenant :** un espace personnel ou une organisation Delino. Delcore facture l’utilisation au tenant.
- **Identifiant d’authentification :** un jeton de connexion Delino, une clé API personnelle ou une clé API de service du tenant. Consultez [Clés API](/fr/services/delitools/api-keys).
- **Identifiant de requête :** chaque réponse contient un identifiant de requête. Incluez-le lorsque vous contactez l’assistance.

## Connexion et autorisations

Envoyez un en-tête `Authorization: Bearer <credential>` avec chaque requête.

| Identifiant d’authentification | Format                          | Tenant                                                     |
| ------------------------------ | ------------------------------- | ---------------------------------------------------------- |
| Jeton de connexion Delino      | Un jeton d’accès Delino         | Définissez `tenantId` dans la requête. Il est obligatoire. |
| Clé API personnelle            | Un secret commençant par `dlk_` | Le tenant de la clé. `tenantId` est facultatif.            |
| Clé de service du tenant       | Un secret commençant par `dlk_` | Le tenant de la clé. `tenantId` est facultatif.            |

Si vous définissez `tenantId` avec une clé API, il doit correspondre au tenant de cette clé, sinon la requête échoue avec `ERROR_REASON_PERMISSION_DENIED`. Chaque requête doit être autorisée par Delcore au moment où elle est effectuée : l’identifiant d’authentification doit disposer de la capacité `translate` ou `extract`, le tenant doit être actif et satisfaire aux vérifications habituelles de facturation. Ces API sont payantes, et Delcore les bloque lorsque votre tenant rencontre un problème de facturation, tel qu’un solde épuisé.

## Démarrage rapide

1. **Obtenez un identifiant d’authentification.** Utilisez votre jeton de connexion Delino ou créez une clé API comme décrit dans [Clés API](/fr/services/delitools/api-keys).
2. **Trouvez l’identifiant de votre tenant et un modèle.** L’identifiant du tenant fait partie de votre compte Delino. L’opérateur du service définit les modèles de traduction disponibles.
3. **Appelez l’API.** Toutes les procédures se trouvent sur `delitools.v1.DelitoolsService`. Définissez `DELITOOLS_URL` sur l’URL de base du serveur d’API DeliTools, par exemple `http://127.0.0.1:46322` pour un serveur local.

```sh
curl --request POST "$DELITOOLS_URL/delitools.v1.DelitoolsService/Translate" \
  --header "Content-Type: application/json" \
  --header "Authorization: Bearer $DELITOOLS_KEY" \
  --data '{"texts":["Hello","Good morning"],"targetLanguage":"fr","model":"<model-id>"}'
```

En JSON, les noms de champs utilisent la notation lowerCamelCase. Consultez la [Référence de l’API](/fr/services/delitools/api-reference) pour connaître tous les champs, le sous-ensemble de schéma pris en charge et les motifs d’erreur.

Appelez DeliTools depuis du code côté serveur ou la ligne de commande. Ne placez pas de clé API dans le code frontend : DeliTools ne prend pas en charge les appels depuis les navigateurs.

## Votre contenu et le fournisseur d’IA

DeliTools exécute votre requête sur Vercel AI Gateway. **Les textes et les schémas que vous soumettez sont envoyés à Vercel AI Gateway et au fournisseur du modèle que vous choisissez.** Le traitement de votre contenu par ces fournisseurs, y compris toute conservation éventuelle, est régi par leurs propres conditions et paramètres, et non par DeliTools.

- DeliTools ne stocke pas vos textes, vos schémas ni vos résultats et ne les écrit pas dans ses journaux. Cela concerne uniquement DeliTools. Il ne s’agit pas d’une garantie quant à ce que Vercel AI Gateway ou le fournisseur du modèle conserve.
- Ne soumettez pas de contenu que vous n’êtes pas autorisé à partager avec ces fournisseurs. Consultez leurs conditions avant de soumettre des données sensibles ou réglementées.
- DeliTools conserve uniquement les enregistrements d’utilisation nécessaires à la facturation de votre tenant et les supprime 30 jours après leur création.
- L’exactitude des résultats du modèle n’est pas garantie. Vérifiez les résultats avant de vous y fier.

## Autres guides

- [Référence de l’API](/fr/services/delitools/api-reference) : champs, balises de langue, sous-ensemble de schéma, erreurs et limites.
- [Clés API](/fr/services/delitools/api-keys) : clés personnelles et de tenant, autorisations, expiration et révocation.
- [Facturation](/fr/services/delitools/billing) : comment l’utilisation de jetons se traduit en frais.
- [Dépannage](/fr/services/delitools/troubleshooting) : configuration du client et résolution des erreurs.
