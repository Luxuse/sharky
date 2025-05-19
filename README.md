
![icons8-shark-100](https://github.com/user-attachments/assets/117735a3-ab9d-4908-950b-e8edd0387b33)


# Sharky Compressor

Sharky est un outil de compression/décompression en ligne de commande développé en Rust. Il est conçu pour gérer efficacement l'archivage et la compression de fichiers et de répertoires en utilisant un pipeline hybride moderne et performant, optimisé pour un bon équilibre entre vitesse et taux de compression.

## Fonctionnalités

* **Compression Hybride :** Utilise une combinaison de **Tar** pour l'archivage, **Deflate (niveau 9)** pour une première passe de compression intermédaire, et **Zstd (niveau configurable)** pour la compression principale.
* **Streaming Efficace :** Traite les données en continu, ce qui permet de compresser ou décompresser de très grands fichiers et répertoires sans utiliser excessivement de mémoire vive.
* **Performances Tunables :** Le niveau de compression Zstd peut être ajusté pour privilégier la vitesse (niveaux bas) ou le taux de compression (niveaux élevés).
* **Indicateur de Progression :** Fournit un retour visuel pendant les opérations de compression et de décompression.

## Méthode de Compression : Tar + Deflate (N9) + Zstd (Niveau Variable)

Lorsque vous compressez avec Sharky (`-c`), le pipeline de données est le suivant :

1.  L'entrée (fichier ou répertoire) est d'abord archivée en un flux unique par **Tar**.
2.  Ce flux est compressé une première fois par l'algorithme **Deflate** au niveau **9** (niveau maximum pour Deflate, fixé dans cette version).
3.  Le flux déjà compressé par Deflate est ensuite compressé une seconde fois par l'algorithme **Zstd** au niveau spécifié par l'utilisateur (`-l`).
4.  Le résultat final est écrit dans le fichier de sortie.

La décompression (`-d`) inverse ce processus :

1.  Le fichier compressé est lu.
2.  Le flux est décompressé par **Zstd**.
3.  Le flux est décompressé par **Deflate**.
4.  Le flux Tar est extrait vers le répertoire de destination.

Cette approche hybride vise à obtenir un bon taux de compression grâce à la force de Zstd, tout en bénéficiant potentiellement d'une performance optimisée grâce à la passe intermédaire Deflate de niveau 9.

## Prérequis

* [Rust](https://www.rust-lang.org/tools/install)
* Cargo (installé avec Rust)

## Construction

1.  Clonez ce dépôt ou téléchargez les fichiers source.
2.  Ouvrez un terminal dans le répertoire racine du projet.
3.  Exécutez la commande suivante pour compiler le projet en mode release (optimisé) :

```bash
cargo build --release
