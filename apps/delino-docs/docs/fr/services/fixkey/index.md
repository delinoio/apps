# FixKey

FixKey corrige les mots saisis avec la mauvaise disposition de clavier. Il prend en charge le clavier coréen Dubeolsik et le clavier anglais américain QWERTY dans les deux sens. FixKey s’exécute localement sur votre ordinateur et ne nécessite aucun compte.

## Installer FixKey

Ouvrez la page des [versions de FixKey](https://github.com/delinoio/apps/releases?q=fixkey%40) et choisissez le paquet correspondant à votre système d’exploitation et à votre processeur :

- macOS ARM64 : DMG pour les Mac avec puce Apple. Déplacez FixKey dans Applications et ouvrez-le.
- Windows x64 ou ARM64 : programme d’installation EXE correspondant à votre processeur. Les paquets Windows ne disposent pas d’une signature d’éditeur Authenticode. Windows peut donc afficher un avertissement concernant un éditeur inconnu ou SmartScreen.
- Linux X11 x64 ou ARM64 : AppImage correspondant à votre processeur. Wayland n’est pas pris en charge.

La configuration d’exécution minimale requise est macOS 11 sur une puce Apple ; Windows 10 avec WebView2 ; ou Linux avec X11, GTK 3, WebKitGTK 4.1 et un bus d’accessibilité AT-SPI. Il s’agit de prérequis propres à chaque plateforme. Ils ne garantissent pas la compatibilité avec toutes les applications ni tous les champs de texte.

## Accorder les autorisations

FixKey a besoin des autorisations d’accessibilité et de saisie de votre système d’exploitation pour lire et remplacer le mot actuel dans un champ modifiable. Suivez la boîte de dialogue d’autorisation, accordez l’accès demandé dans les réglages du système, puis choisissez **Revérifier**. La correction reste inactive tant que les autorisations requises et les dictionnaires intégrés ne sont pas prêts. Les champs de mot de passe et les champs protégés sont exclus.

Sous macOS, vérifiez **Accessibilité** et **Surveillance de l’entrée** dans Réglages Système. L’autorisation d’accessibilité peut accorder un accès effectif à la saisie sans entrée distincte dans Surveillance de l’entrée. Pour vérifier cette liste, ouvrez **Général** dans FixKey et choisissez **Configurer la surveillance de l’entrée**. Si FixKey n’y figure pas, utilisez **+** dans la liste des autorisations du système pour ajouter l’application FixKey.app actuelle depuis Applications, puis revenez dans FixKey et effectuez une nouvelle vérification. Une entrée distincte dans la liste n’est pas nécessaire lorsque macOS accorde déjà un accès effectif à la saisie.

## Corriger un mot

Les deux dispositions sont activées par défaut. Saisissez du texte dans un champ modifiable et terminez un mot par une espace ou un signe de ponctuation. FixKey compare les candidats des dispositions activées à ses dictionnaires intégrés. Il remplace le mot uniquement lorsqu’une seule langue activée reconnaît son candidat et que ce candidat diffère du mot d’origine. Si aucune langue ou plusieurs langues reconnaissent un candidat, le mot reste inchangé.

Par exemple, les séquences de touches `rPthrgo` et `zlqhemrk` peuvent devenir `계속해` et `키보드가`. Ces exemples illustrent la correction de disposition ; les résultats dépendent du champ actuel, du contexte de saisie et de la reconnaissance par les dictionnaires.

FixKey conserve le séparateur qui suit un mot corrigé. Entrée et Tabulation ne déclenchent pas la correction. La ponctuation utilisée dans les URL ou les adresses e-mail peut retarder l’évaluation jusqu’à ce que FixKey puisse classer le jeton entier. Les apostrophes et les traits d’union placés entre des lettres restent à l’intérieur du mot.

La correction automatique ignore le texte collé ou inséré par programmation, les nombres, les emoji, les URL, les adresses e-mail et les jetons utilisant plusieurs systèmes d’écriture. Elle attend que la composition de la saisie coréenne soit terminée. Les champs inconnus, les autorisations indisponibles et les champs qui ne peuvent pas être modifiés en toute sécurité restent intacts.

Après une correction automatique réussie sous macOS ou Windows, FixKey essaie également de sélectionner une source de saisie activée correspondant à la langue corrigée. Sous Windows, il peut utiliser les modes hangul et latin de l’IME coréen de Microsoft, même lorsqu’aucune source US QWERTY distincte n’est activée. Gardez votre IME coréen configuré pour Dubeolsik. Si la sélection de la source de saisie est indisponible ou échoue, le texte corrigé et l’annulation immédiate restent disponibles. Les IME Windows fermés ou non pris en charge ne sont pas modifiés. Linux conserve la source de saisie actuelle. L’annulation immédiate ne modifie pas à nouveau la source de saisie.

### Annuler une correction

Appuyez sur Retour arrière immédiatement après une correction automatique pour restaurer le mot d’origine et son séparateur. Les pressions suivantes sur Retour arrière suppriment normalement le texte. Le déplacement du curseur, la modification de la sélection, le passage à un autre champ ou le début d’un autre mot mettent fin à ce contexte d’annulation immédiate.

### Choisir manuellement un candidat

1. Ouvrez **Raccourcis** et attribuez le raccourci de sélection manuelle d’un candidat. Aucun raccourci ne lui est attribué par défaut. Si l’enregistrement échoue, consultez la raison affichée ou choisissez un autre raccourci.
2. Saisissez un mot dans un champ modifiable et utilisez le raccourci.
3. Utilisez les touches fléchées pour sélectionner un candidat. Appuyez sur Entrée pour l’appliquer ou sur Échap pour fermer le sélecteur.

Le sélecteur affiche les résultats de reconnaissance des dictionnaires. Vous pouvez choisir explicitement un candidat non reconnu. Le déplacement du curseur dans le champ source ou le passage à une autre application ferme le sélecteur. Le sélecteur n’est pas disponible lorsque FixKey ne peut pas établir une cible de remplacement sûre.

## Configurer la correction

Dans **Général**, mettez la correction en pause ou reprenez-la, sélectionnez les langues activées, activez le lancement à l’ouverture de session et choisissez la langue ainsi que le thème de l’interface. Le lancement à l’ouverture de session est désactivé par défaut. Les réglages s’appliquent immédiatement et sont enregistrés localement.

Pour qu’une application reste inchangée, ajoutez-la sous **Applications exclues**. Les exclusions identifient l’application elle-même, et pas seulement son nom affiché. Une application non résolue reste répertoriée afin que vous puissiez l’examiner.

## Confidentialité et utilisation hors ligne

FixKey traite les saisies sur votre appareil. Il n’enregistre ni ne transmet jamais le texte saisi, les candidats, le contenu des documents, les titres des fenêtres ou l’historique des corrections. Il n’utilise pas le presse-papiers pour effectuer les remplacements. Il n’y a aucune télémétrie, aucun rapport de plantage à distance et aucun téléversement automatique de données de diagnostic.

Les dictionnaires sont intégrés, de sorte que la correction fonctionne hors ligne dès le premier lancement, une fois les autorisations prêtes. Les dictionnaires évoluent avec les mises à jour de l’application ; il n’existe ni téléchargement distinct des dictionnaires ni éditeur de dictionnaire utilisateur. L’utilisation du réseau se limite à la recherche et au téléchargement de mises à jour.

## Mettre à jour FixKey

FixKey recherche des mises à jour stables au démarrage, toutes les 24 heures pendant son exécution et lorsque vous demandez une vérification dans **Mises à jour**. Le téléchargement et l’installation nécessitent votre consentement. Les signatures des paquets de mise à jour sont vérifiées, et toute signature non valide empêche l’installation. La correction s’arrête avant l’installation et les réglages sont conservés.

Après l’échec d’une vérification, réessayez manuellement ou attendez la prochaine vérification planifiée. Si l’installation échoue, redémarrez FixKey ou réinstallez le paquet approprié depuis la page des versions de FixKey. Vous pouvez réinstaller manuellement une version antérieure ; FixKey ne revient jamais automatiquement à une version précédente.

## Dépannage et assistance

Si la correction est inactive, ouvrez **Diagnostic et aide**, puis consultez l’état et le code de motif. Vérifiez à nouveau les autorisations et essayez un champ modifiable dans une autre application. Un état indiquant l’attente d’un champ modifiable signifie que le champ actuel n’a pas pu être identifié en toute sécurité. Vérifiez également si la correction est en pause ou si l’application est exclue.

Utilisez **Ouvrir le dossier des journaux** pour consulter les journaux de diagnostic locaux ou **Supprimer les journaux** pour les effacer. Les journaux contiennent les résultats de diagnostic et les codes de motif, mais pas le contenu des saisies. FixKey conserve au maximum sept jours de journaux, pour une taille totale maximale de 10 Mio.

Pour obtenir de l’aide, utilisez le lien GitHub Issues dans **Diagnostic et aide** ou ouvrez [GitHub Issues](https://github.com/delinoio/apps/issues). Indiquez la version de FixKey, le système d’exploitation, l’application concernée et le code de motif. N’incluez aucun texte privé ni identifiant de connexion.
