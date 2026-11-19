<p align="center">
  <img src="packaging/icons/icon.png" width="96" alt="Orvyn">
</p>
<h1 align="center">Orvyn</h1>

<p align="center">
  <b>Un atelier puissant pour vos bases de données. La vitesse native de Rust.</b><br>
  35 sources de données. SQL, NoSQL, recherche et analyse dans un même espace de travail.
</p>

<p align="center">
  <a href="https://github.com/dbx192/Orvyn/releases/latest"><img src="https://img.shields.io/github/v/tag/dbx192/Orvyn?label=release&amp;color=teal" alt="Dernière version"></a>
  <img src="https://img.shields.io/badge/built_with-Rust-orange.svg" alt="Développé en Rust">
  <img src="https://img.shields.io/badge/desktop-Windows%20%7C%20Linux%20%7C%20macOS-0078D4.svg" alt="Windows, Linux, macOS">
  <img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="Licence MIT">
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="README.hi.md">हिन्दी</a> · <a href="README.es.md">Español</a> · <b>Français</b>
</p>

---

**Orvyn est un atelier de bases de données haute performance, développé en Rust avec egui.** Écrivez du SQL, modifiez des lignes, concevez des schémas, gérez des clés Redis, interrogez MongoDB, explorez des index de recherche et exportez de grandes tables depuis une application de bureau native.

Des bases de production à l'analyse locale avec DuckDB, des pilotes natifs, des intégrations HTTP et des passerelles ODBC relient **35 types de sources de données**. SQLite est intégré et les paquets de bureau incluent DuckDB 1.5.4.

**Exécution native. Grilles virtualisées. Export en flux. Un espace de travail ouvert sur le monde.**

## La performance dans l'architecture

| Technologie | Bénéfice |
| --- | --- |
| **Rust + egui natifs** | Une application compilée, sans Electron ni moteur de navigateur. |
| **Tokio en arrière-plan** | Les tâches de base de données s'exécutent hors du fil d'interface ; les opérations bloquantes peuvent utiliser un groupe de travail à concurrence limitée. |
| **Grilles virtualisées** | Les vues de résultats et de tables ne dessinent que les lignes visibles. |
| **Pagination, filtrage et tri côté serveur** | Parcourez des pages de taille limitée et déléguez les filtres de table au serveur. |
| **Connexions réutilisées** | Les pilotes MySQL et PostgreSQL disposent de pools de connexions. |
| **Export par lots de 5 000 lignes** | Les pilotes de protocole MySQL/PostgreSQL comptent une fois, puis parcourent les données en un seul flux, sans pagination de l'éditeur ni historique de requêtes. |
| **Écriture incrémentale** | CSV, JSON, TSV et XLSX sont écrits progressivement ; les feuilles XLSX utilisent une mémoire constante et sont réparties à la limite de lignes. |
| **Coloration SQL temporisée** | Les modifications rapides sont regroupées ; la coloration est désactivée au-delà du seuil des grands documents. |

L'export de tables affiche la progression, permet l'annulation, empêche les exports de tables simultanés et utilise des fichiers temporaires pour préserver une destination existante en cas d'annulation. Les autres pilotes relationnels utilisent un export paginé. Le débit dépend du serveur, du plan d'exécution, du réseau, de la taille des lignes et du format.

## Cinq langues, un même espace de travail

**English · 简体中文 · हिन्दी · Español · Français**

Les cinq langues d'interface sont disponibles dans la même application. Au premier lancement, Orvyn choisit la langue du système lorsqu'elle est prise en charge. Vous pouvez la modifier dans l'interface ; votre choix est conservé pour les prochains lancements. Les thèmes clair et sombre et les polices intégrées **Inter, JetBrains Mono et Noto Sans CJK** complètent un environnement adapté aux équipes internationales.

## Sources de données

| Famille | Sources prises en charge |
| --- | --- |
| **Protocole MySQL · 8** | MySQL · MariaDB · TiDB · OceanBase · Apache Doris · SelectDB · StarRocks · GoldenDB |
| **Protocole PostgreSQL · 11** | PostgreSQL · TimescaleDB · Redshift · CockroachDB · openGauss · GaussDB · KWDB · KingbaseES · Vastbase · HighGo · UXDB |
| **Moteurs relationnels et analytiques · 5** | SQLite · DuckDB · SQL Server · ClickHouse · TDengine (REST) |
| **Documents, clé-valeur et HTTP SQL · 3** | MongoDB · Redis · Cloudflare D1 |
| **Recherche · 4** | Elasticsearch · Easysearch · Meilisearch · Manticore Search |
| **ODBC fournisseur · 4** | Oracle · DM (达梦) · XuguDB (虚谷) · Microsoft Access |

Les familles de protocoles partagent des pilotes tout en conservant les particularités des moteurs. ODBC nécessite un pilote compatible installé sur votre système. Les fonctions dépendent du moteur, de sa version et des autorisations.

## Les outils du quotidien

- **Atelier SQL :** onglets multiples, coloration, formatage et complétion des mots-clés, tables et colonnes avec reconnaissance des alias. Exécutez l'instruction courante, une sélection ou tout le script avec résultats distincts, durée et lignes affectées. Historique, favoris, transactions et annulation selon le moteur ; MySQL/PostgreSQL permettent l'annulation côté serveur lorsque les capacités et autorisations le permettent.
- **Exploration et édition :** arbre des bases, schémas, tables, vues, colonnes, index, clés étrangères, déclencheurs et routines. Pagination, filtres, tri et édition par clé primaire, avec modifications préparées et confirmation avant envoi.
- **Conception des schémas :** préparez les ajouts, suppressions, renommages et changements de type de colonnes. Gérez clés primaires, index et clés étrangères avec du SQL adapté au moteur. Modifiez les déclencheurs et routines en conservant les signatures de surcharge PostgreSQL.
- **Import et export :** CSV, JSON, TSV compatible Excel et XLSX natif. Les grands entiers et décimaux restent du texte dans XLSX pour préserver leur précision. Export SQL du schéma seul ou avec données sur les moteurs compatibles. Assistant CSV avec détection d'encodage, aperçu, insertions par lots, progression, annulation et rapport des lignes en échec.
- **Redis :** scan par curseur, vue plate ou hiérarchique, édition String/Hash/List/Set/Sorted Set, création, renommage, suppression par lots, TTL, affichage JSON/brut et console de commandes.
- **MongoDB :** filtres, projection, tri, pagination, agrégations et création, modification ou suppression de documents individuels.
- **Recherche et D1 :** Elasticsearch/Easysearch avec complétion fondée sur les mappings, réponses brutes, pagination profonde `PIT` / `search_after` et export JSON de la page courante ; navigation Meilisearch/Manticore ; SQL HTTP Cloudflare D1 avec résultats distincts par instruction.

## Sécurité et maîtrise des changements

Les identifiants enregistrés sont chiffrés avec **ChaCha20-Poly1305**. L'export des connexions masque les mots de passe par défaut. Cinq modes SSL (`Disable`, `Prefer`, `Require`, `VerifyCa`, `VerifyFull`, selon le connecteur) et un mode lecture seule complètent la protection. Verrouillage des fichiers et sauvegardes atomiques protègent la configuration ; une clé maîtresse vide ou illisible n'est jamais remplacée silencieusement.

## Télécharger et démarrer

Les paquets sont disponibles sur **[GitHub Releases](https://github.com/dbx192/Orvyn/releases/latest)**.

| Plateforme | Paquet du workflow de publication |
| --- | --- |
| Windows x86_64 | Installateur Inno Setup par utilisateur, sans élévation administrateur. |
| Linux x86_64 / amd64 | Paquet `.deb` avec la bibliothèque DuckDB. |
| macOS Intel x86_64 | Application `.app` dans un `.dmg`, avec DuckDB. |
| macOS Apple Silicon aarch64 | Application `.app` dans un `.dmg`, avec DuckDB. |

Les scripts Linux et de préparation DuckDB gèrent aussi ARM64 ; le workflow actuel publie Linux x86_64.

1. Installez Orvyn et créez une connexion du type souhaité.
2. Renseignez les paramètres ou le chemin de la base locale, testez et enregistrez.
3. Explorez les objets, ouvrez une requête ou utilisez les navigateurs Redis et de recherche.

Raccourcis Windows/Linux : `Ctrl+N` nouvelle requête, `Ctrl+Enter` instruction courante, `Ctrl+Shift+Enter` sélection, `Ctrl+Shift+F` formatage, `Ctrl+L` recherche d'objets et `Ctrl+T` thème. Sur macOS, utilisez `Cmd`.

## Compiler depuis les sources

Prérequis : Rust stable (minimum déclaré 1.88 ; CI en 1.96.1), Python 3 et outils natifs. Utilisez la version Rust du CI pour reproduire les versions publiées.

- **Windows :** Visual Studio C++ Build Tools et Rust MSVC.
- **Linux :** `build-essential pkg-config unixodbc-dev libgtk-3-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libwayland-dev`.
- **macOS :** outils en ligne de commande Xcode et `brew install unixodbc`.

Depuis la racine du dépôt :

```sh
python packaging/prepare-duckdb.py
cargo run -p orvyn --release --locked
```

Vérifications du projet :

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

DuckDB 1.5.4 est téléchargé via `https://gh-proxy.org/` dans `packaging/duckdb-download/local/1.5.4/`. Définissez `ORVYN_GITHUB_PROXY` sur une chaîne vide pour accéder directement à GitHub, ou fournissez un autre préfixe de proxy.

Empaquetage : [Windows](packaging/windows/build-installer.ps1) (Inno Setup 6), [Linux](packaging/linux/package.sh), [macOS](packaging/macos/package.sh). Le [workflow de publication](.github/workflows/ci.yml) s'exécute sur les tags `v*` : tests natifs et Clippy sur trois systèmes, intégration PostgreSQL/MySQL/Redis/Elasticsearch, puis quatre compilations et publication.

## Configuration

| Système | Répertoire |
| --- | --- |
| Windows | `%APPDATA%\Orvyn` |
| Linux | `$XDG_CONFIG_HOME/orvyn` ou `~/.config/orvyn` |
| macOS | `~/Library/Application Support/Orvyn` |

`ORVYN_CONFIG_DIR` remplace le répertoire. `ORVYN_MASTER_KEY` fournit la clé de chiffrement ; sinon, un fichier privé `master.key` est créé. `RUST_LOG` contrôle le journal `orvyn.log` dans le répertoire de configuration. **Sauvegardez la clé avec les identifiants chiffrés : leur restauration nécessite la même clé.**

## Compatibilité

- ClickHouse, TDengine, Cloudflare D1 et MongoDB n'exposent pas de contrôles de transaction côté client.
- L'annulation ne supprime pas le travail déjà validé ; certains connecteurs interrompent seulement l'attente du client. Les diagnostics distinguent ce cas d'une annulation confirmée côté serveur.
- Le mode lecture seule est une protection côté client ; utilisez les permissions de la base pour limiter les effets des fonctions serveur.
- Les tunnels SSH, la comparaison de schémas/données, les diagrammes ER, l'administration des utilisateurs/permissions et les mises à jour automatiques ne sont pas encore implémentés.

## Licence

Distribué sous [licence MIT](LICENSE).
