# Rustcraft

**Minecraft: Java Edition 26.3 réécrit en Rust : le client et son serveur intégré, pour jouer au même jeu, en plus rapide.**

![Rust 1.99](https://img.shields.io/badge/Rust-1.99-orange?logo=rust)
![Minecraft Java 26.3](https://img.shields.io/badge/Minecraft_Java-26.3-62B47A)
![Protocole 777](https://img.shields.io/badge/protocole-777-blue)
![wgpu : Vulkan ou DX12](https://img.shields.io/badge/rendu-Vulkan_%7C_DX12-AC162C?logo=vulkan)
![Windows et Linux](https://img.shields.io/badge/plateforme-Windows_%7C_Linux-0078D6)

[English version](README.md)

Rustcraft remplace le client Java du jeu par un programme natif. Tu le lances depuis le **lanceur
officiel**, avec ton compte, et tu rejoins les mêmes serveurs (vanilla, Paper…) ou tu joues tes
propres mondes en solo. Le jeu lui-même doit rester le même : mêmes règles, même génération du
monde, même protocole. Ce qui change, c'est le moteur : pas de JVM, pas de ramasse-miettes, tous les
cœurs du processeur au travail, et un rendu GPU moderne.

> [!WARNING]
> Projet non officiel et **en cours de développement**. Le multijoueur et le solo sont jouables,
> mais des parties du jeu manquent encore ou n'ont pas encore été vérifiées contre le jeu officiel.
> Lis [État du projet](#état-du-projet) avant de t'y fier, et garde des sauvegardes de tes mondes.

## État du projet

Une seule règle : partout où le jeu officiel a un comportement, c'est lui la spécification. Un audit
complet du code du jeu a transformé chaque écart connu en mission ; les chiffres ci-dessous viennent
de cette liste.

| Domaine | Où on en est |
|---|---|
| Fidélité au jeu officiel | **53 missions d'audit sur 83** corrigées et intégrées, 8 autres faites et en relecture |
| Optimisation | **47 optimisations sur 125** prévues faites, chacune mesurée |
| Tests | environ **2 200** tests automatiques, dont beaucoup comparés bit à bit avec le jeu officiel |

### Ce qui marche bien

- **Multijoueur**, hors ligne et en ligne : compte Microsoft par le lanceur officiel, chiffrement,
  compression, chat signé, packs de ressources imposés par les serveurs, réseau local.
- **Solo** sur un serveur intégré écrit en Rust : créer, ouvrir, jouer et sauvegarder des mondes au
  format officiel.
- **Génération du monde** identique au jeu officiel pour une même graine (bruits, biomes, terrain et
  décoration vérifiés contre lui par des tests de référence).
- **Éclairage** identique au jeu officiel, ainsi que la **physique** du joueur et des montures.
- **Redstone**, avec les bizarreries du jeu officiel reproduites volontairement (quasi-connectivité,
  ordre des mises à jour), et les bogues que Mojang garde, listés dans [docs/quirks-to-reproduce.md](docs/quirks-to-reproduce.md).
- **Créatures et villageois** (IA, commerce, reproduction, raids), blocs et blocs-entités, objets,
  enchantements, effets, commandes avec autocomplétion.
- **Interface** : écran titre, liste des serveurs, HUD, inventaires, inventaire créatif, livre de
  recettes, chat, liste des joueurs, tableaux des scores, barres de boss, écran F3, options et
  `options.txt` partagés avec le jeu officiel ; sons, musique et particules.

### En cours

Faits et en attente d'intégration : laisse et cochon sellé, contacts des entités (feu, pluie, magma,
baies, miel, chutes, usure de l'armure), poudre d'os, blocs de soufre et projectiles, golems, effets
des blocs-entités et des créatures côté client.

Encore sur la liste de fidélité, par priorité :

- combat et enchantements offensifs ; orbes d'expérience, objets au sol et entités d'affichage ;
- ticks de la nature et œufs de tortue ; blocs techniques (structure, jigsaw, test), `/place`, `/clone`, `/locate` ;
- quelques commandes (`/loot`, `/enchant`, attributs…), tags des packs de données et `/reload`, sauvegarde, `/schedule` et `/data` ;
- apparitions et œufs d'apparition, navigation et cartes, barre de localisation ;
- écrans F3 et de débogage, écrans d'inventaire et menus d'options, petits effets côté client.

Faits mais **pas encore comparés systématiquement** au jeu officiel : écoulement des fluides,
mouvement et collisions des entités, chaîne de dégâts, faim, clics génériques des menus, lignes
d'attributs des infobulles.

### Manques connus

- **macOS** n'est pas encore pris en charge (prévu en dernier).
- **Linux** compile et se lance, mais a très peu servi en vraie partie. Il faut une carte graphique
  avec un pilote Vulkan : il n'y a pas de repli OpenGL, les vieilles puces intégrées tombent sur un
  rendu logiciel.
- Pas de **narrateur**, pas de **packs de shaders**, et les packs de ressources qui remplacent les
  shaders du jeu ne sont pas pris en charge.
- Les mondes des **anciennes versions** du jeu ne sont pas convertis : ouvre-les d'abord avec la 26.3.
- Le **serveur dédié** est prévu, après le client.
- Pas prévus : Realms, télémétrie, RCON et le protocole de gestion JSON-RPC, et la connexion à un
  compte Microsoft dans Rustcraft (le lanceur officiel s'en charge ; Rustcraft ne voit jamais ton
  mot de passe).

Chaque écart trouvé jusqu'ici est listé avec sa cause dans [docs/gaps-audit2.md](docs/gaps-audit2.md),
[docs/gaps-client.md](docs/gaps-client.md) et [docs/gaps-protocol.md](docs/gaps-protocol.md).

## Performances

Rustcraft contre le client officiel 26.3, sur la même machine (Ryzen 9 7950X, 32 Go, RTX 5080,
Windows 11), en fenêtre 1920×1080 avec le même `options.txt` (synchro verticale coupée, images par
seconde illimitées). Les deux jeux ouvrent le même monde neuf (graine 1234) et volent 192 blocs à
16 blocs par seconde au-dessus d'un terrain généré en route, à quatre endroits : un village, une
forêt, l'océan et des montagnes. Chaque vol est fait trois fois ; les tableaux donnent l'écart entre
les quatre endroits.

| Distance de rendu 16 | Client officiel | Rustcraft |
|---|---|---|
| Images par seconde, moyenne | 1 389 à 1 712 | **2 601 à 3 197** |
| 1 % d'images le plus lent | 328 à 348 par seconde | **895 à 1 310 par seconde** |
| Image la plus longue | 10,8 à 15,1 ms | **2,4 à 3,4 ms** |
| Cœurs du processeur occupés | 3,1 à 4,3 | **1,5 à 1,7** |
| Mémoire (ensemble de travail) | 2 190 à 2 390 Mo | **1 360 à 1 590 Mo** |
| Du lancement à l'entrée dans le monde | 11,9 à 12,8 s | **0,9 à 1,2 s** |

| Distance de rendu 32 | Client officiel | Rustcraft |
|---|---|---|
| Images par seconde, moyenne | 863 à 1 454 | **1 849 à 2 633** |
| 1 % d'images le plus lent | 101 à 195 par seconde | **460 à 785 par seconde** |
| Image la plus longue | 95 à 258 ms | **3,7 à 9,9 ms** |
| Cœurs du processeur occupés | 6,6 à 8,0 | **2,0 à 2,3** |
| Mémoire (ensemble de travail) | 2 800 à 2 890 Mo | **1 980 à 2 480 Mo** |
| Du lancement à l'entrée dans le monde | 11,9 à 13,0 s | **0,8 à 3,1 s** |

À l'arrivée dans le monde : le joueur reste où il est entré et la fenêtre du jeu est filmée dix fois
par seconde ; la vue est complète quand l'image ne change plus (interface, vignette et nuages mis de
côté, de la même façon pour les deux jeux).

| Vue complète, monde neuf | Client officiel | Rustcraft |
|---|---|---|
| Après l'entrée, distance de rendu 16 | 8,0 à 15,3 s | **0,9 à 1,4 s** |
| Après l'entrée, distance de rendu 32 | 22,4 à 35,5 s | **2,6 à 4,3 s** |
| Depuis le lancement, distance de rendu 16 | 20,0 à 27,2 s | **1,9 à 2,2 s** |
| Depuis le lancement, distance de rendu 32 | 35,1 à 48,0 s | **4,8 à 6,3 s** |

Les durées d'image viennent de [PresentMon](https://github.com/GameTechDev/PresentMon) pour les deux
jeux, la mémoire et le temps processeur des compteurs du processus. Le client officiel tourne sur le
Java du lanceur, sous un autre nom de fichier, car le profil du pilote graphique pour `java.exe` le
bride à la fréquence de l'écran. L'outil est [tools/bench/official_compare.py](tools/bench/official_compare.py).
Chaque optimisation est aussi mesurée seule, avant et après, dans le [journal des mesures](docs/perf.md).

Ce qui fait la différence :

- **Tout en parallèle** : génération, éclairage, maillage et décodage réseau se répartissent sur tous
  les cœurs, sans s'attendre.
- **Un rendu GPU moderne** : un seul tampon de faces, des dessins indirects fusionnés, une visibilité
  triée par section, aucune allocation par image en régime établi.
- **Démarrage instantané** : modèles, textures, polices et sons sont cuits une fois dans un cache binaire.
- **Mêmes résultats** : une optimisation n'est gardée que si son résultat est identique à l'ancien
  chemin, ce que vérifie un test qui échoue quand on sabote l'optimisation.

## Installation

Il te faut :

- **Minecraft: Java Edition** (le jeu acheté sur ton compte Microsoft) et le **lanceur officiel** ;
- **Windows 10 ou 11** (64 bits) avec une carte graphique **Vulkan** ou **DirectX 12**, ou **Linux**
  (x86-64, glibc 2.35 ou plus récente) avec un pilote **Vulkan**.

### 1. Préparer le jeu officiel

Dans le lanceur, lance **une fois** la version officielle **26.3**, puis ferme-la. Le lanceur
télécharge alors les fichiers du jeu, que Rustcraft lit sur ta machine.

### 2. Obtenir Rustcraft

Télécharge-le depuis la page [Releases](https://github.com/neotoxicfr/rustcraft/releases) quand une
release correspond à ta version, ou [compile-le](#compiler). Range-le dans un dossier qui ne bouge
pas, par exemple `%LOCALAPPDATA%\rustcraft\bin\rustcraft.exe` sous Windows, ou
`~/.local/share/rustcraft/bin/` sous Linux (garde `rustcraft` et `rustcraft.sh` ensemble).

### 3. L'ajouter au lanceur

1. Dans le lanceur, ouvre **Installations**, puis **Nouvelle installation**.
2. **Nom** : `Rustcraft`. **Version** : `release 26.3`.
3. Ouvre **Plus d'options**. Dans **Exécutable Java**, choisis `rustcraft.exe` sous Windows, ou
   `rustcraft.sh` sous Linux.
4. Clique sur **Créer**, puis sur **Jouer**.

Le lanceur démarre alors Rustcraft à la place de Java et lui passe ton pseudo, ton identifiant et ton
jeton de session, comme au jeu officiel ; les options Java qu'il ajoute sont ignorées. Le tout
premier lancement prépare les données de la version : il télécharge le serveur officiel depuis les
serveurs de Mojang, vérifie son empreinte, et en extrait les tables du jeu avec le Java du lanceur.
Ça prend quelques minutes, une seule fois.

Rustcraft partage le dossier du jeu officiel pour la liste des serveurs, `options.txt` et les packs
de ressources. Sous Linux, les mondes et le cache vont dans `~/.local/share/rustcraft` et
`~/.cache/rustcraft`.

## Compiler

Il te faut Rust (la version est fixée par `rust-toolchain.toml` et installée par `rustup`) et, sous
Windows, les outils de compilation C++ de Visual Studio.

```
git clone https://github.com/neotoxicfr/rustcraft.git
cd rustcraft
cargo build --release -p rc-app
```

Le programme est `target/release/rustcraft.exe` (`rustcraft` sous Linux, où la compilation demande
aussi les paquets de développement d'ALSA et d'udev). Le script de lancement Linux est dans
[packaging/linux](packaging/linux).

## Organisation du dépôt

| Crate | Ce qu'elle porte |
|---|---|
| `rc-app` ([`apps/client`](apps/client)) | le client : fenêtre, rendu, interface, boucle de jeu |
| `rc-sim`, `rc-light` | la simulation du serveur intégré, l'éclairage |
| `rc-worldgen` | la génération du monde |
| `rc-net`, `rc-proto`, `rc-proto-v777`, `rc-client` | réseau, chiffrement, protocole 777 |
| `rc-world`, `rc-mesh`, `rc-physics`, `rc-env` | données du monde, maillage, physique, environnement |
| `rc-assets`, `rc-data` | ressources et données lues dans l'installation du joueur |
| `rc-text`, `rc-audio`, `rc-commands` | texte et traductions, son, arbre des commandes |

La documentation est dans [`docs/`](docs) : [architecture](docs/architecture.md),
[journal des mesures](docs/perf.md), [protocole d'optimisation](docs/optimization-protocol.md), et
59 spécifications du comportement du jeu rédigées dans nos propres mots dans [`docs/specs/`](docs/specs).

## Mentions légales

**NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR MICROSOFT.**
Rustcraft n'est pas un produit officiel Minecraft. Il n'est ni approuvé par Mojang ou Microsoft, ni
associé à eux.

- « Minecraft » est une marque de Mojang Synergies AB et de Microsoft ; le nom n'est utilisé ici que
  pour dire avec quel jeu Rustcraft fonctionne.
- **Tu dois posséder Minecraft: Java Edition.** Rustcraft fonctionne uniquement avec une installation
  officielle et un compte légitime.
- **Ce dépôt ne contient ni ne distribue rien de Mojang** : ni code, ni texture, ni son, ni modèle,
  ni donnée du jeu. Tout est lu sur ta machine depuis ton installation officielle ; le serveur
  officiel est téléchargé par ta machine depuis les serveurs de Mojang, comme le fait le lanceur, et
  son empreinte est vérifiée.
- Aucune ligne de code de Mojang ne figure dans ce dépôt. Le comportement du jeu est décrit dans nos
  propres mots dans [`docs/specs/`](docs/specs), puis implémenté à partir de ces descriptions.
- Ton utilisation du jeu reste soumise au [CLUF de Minecraft](https://www.minecraft.net/eula) et aux
  [règles d'utilisation](https://www.minecraft.net/usage-guidelines).
- **Ton compte** : Rustcraft ne demande jamais ton mot de passe. Il reçoit du lanceur officiel un
  jeton de session, le garde en mémoire seulement, ne l'écrit dans aucun journal et ne l'envoie
  qu'aux services de session de Mojang.
- **Les serveurs** : Rustcraft s'annonce aux serveurs comme le client officiel (`vanilla`) pour être
  traité à l'identique. C'est tout de même un client tiers : certains serveurs les interdisent,
  respecte leurs règles.

## Contribuer

Les contributions sont bienvenues ; le code et la documentation sont en anglais. Lis d'abord [CONTRIBUTING.md](CONTRIBUTING.md),
en particulier la règle de la salle blanche : rien de Mojang n'entre dans le dépôt. Pour signaler une
faille, suis [SECURITY.md](SECURITY.md).

## Licence

Le code de Rustcraft est sous double licence, au choix : [Apache 2.0](LICENSE-APACHE) ou
[MIT](LICENSE-MIT). Sauf mention contraire, toute contribution proposée pour inclusion est placée
sous ces deux licences, sans condition supplémentaire. La licence couvre le code de ce dépôt
uniquement, pas le jeu Minecraft ni ses ressources, qui restent la propriété de Mojang.
