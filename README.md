
![MegalodonSU](https://github.com/user-attachments/assets/924cacb4-1d32-4720-8820-ae93659f3b28)


# Sharky Compressor

**Sharky** est un outil de compression/décompression en ligne de commande écrit en Rust. Il offre un pipeline hybride moderne et performant, optimisé pour un équilibre idéal entre vitesse et taux de compression.

## Fonctionnalités

- **Compression hybride**  
  Combine Tar pour l’archivage, XZ pour une première passe de compression, puis Zstd (niveau configurable) pour la compression principale.

- **Streaming efficace**  
  Traite les données en continu, ce qui permet de (dé)compresser de très gros fichiers ou répertoires sans consommer excessivement de mémoire vive.

- **Performances adaptables**  
  Ajustez le niveau de compression Zstd pour privilégier la vitesse (niveaux bas) ou le taux de compression (niveaux élevés).

- **Barre de progression**  
  Affiche l’avancement en temps réel pendant les opérations.

## Méthode de compression

### Compression (`-c`)

1. **Tar**  
   L’entrée (fichier ou répertoire) est archivées en un unique flux Tar.  
2. **XZ**  
   Le flux Tar est compressé via XZ au niveau spécifié (`-x N`).  
3. **Zstd**  
   Le résultat XZ est ensuite compressé via Zstd au niveau choisi (`-z M`).  
4. **Sortie**  
   Le flux final est écrit dans le fichier de sortie.

### Décompression (`-d`)

1. Lecture du fichier compressé.  
2. Décompression Zstd.  
3. Décompression XZ.  
4. Extraction du flux Tar vers le répertoire cible.

> Cette approche hybride combine la rapidité de Zstd et les optimisations de XZ pour maximiser le taux de compression.

## Prérequis

- [Rust](https://www.rust-lang.org/)  
- [Cargo](https://doc.rust-lang.org/cargo/) (installé avec Rust)

## Compilation

1. Clonez ce dépôt ou téléchargez les sources.  
2. Ouvrez un terminal à la racine du projet.  
3. Compilez en mode release :

   ```bash
   cargo build --release
