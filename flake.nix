{
  description = "Visual diff desktop app";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = {self, nixpkgs, ...}: let
    system = "x86_64-linux";
    pkgs = import nixpkgs {inherit system;};
    monoRegular = "${pkgs.jetbrains-mono}/share/fonts/truetype/JetBrainsMono-Regular.ttf";
    monoMedium = "${pkgs.jetbrains-mono}/share/fonts/truetype/JetBrainsMono-Medium.ttf";
    runtimeLibraries = with pkgs; [
      libGL
      libxkbcommon
      vulkan-loader
      wayland
      libx11
      libxcursor
      libxi
      libxrandr
    ];
    desktopItem = pkgs.makeDesktopItem {
      name = "visual-diff";
      desktopName = "Visual Diff";
      comment = "Review asset and JSON diffs in Git repositories";
      exec = "visual-diff";
      categories = ["Development" "Graphics"];
    };
  in {
    packages.${system}.default = pkgs.rustPlatform.buildRustPackage {
      pname = "visual-diff";
      version = "0.1.0";
      src = pkgs.lib.fileset.toSource {
        root = ./.;
        fileset = pkgs.lib.fileset.unions [
          ./Cargo.lock
          ./Cargo.toml
          ./src
        ];
      };
      cargoLock.lockFile = ./Cargo.lock;
      JETBRAINS_MONO_REGULAR = monoRegular;
      JETBRAINS_MONO_MEDIUM = monoMedium;
      nativeBuildInputs = with pkgs; [git makeWrapper pkg-config];
      buildInputs = runtimeLibraries;
      postFixup = ''
        mkdir -p $out/share/applications
        ln -s ${desktopItem}/share/applications/visual-diff.desktop $out/share/applications/
        wrapProgram $out/bin/visual-diff \
          --prefix PATH : ${pkgs.lib.makeBinPath [pkgs.git]} \
          --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath runtimeLibraries}
      '';
    };

    apps.${system}.default = {
      type = "app";
      program = "${self.packages.${system}.default}/bin/visual-diff";
    };

    devShells.${system}.default = pkgs.mkShell {
      packages = with pkgs; [
        cargo
        clippy
        git
        pkg-config
        rust-analyzer
        rustc
        rustfmt
      ];

      buildInputs = runtimeLibraries;

      LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibraries;
      JETBRAINS_MONO_REGULAR = monoRegular;
      JETBRAINS_MONO_MEDIUM = monoMedium;
    };
  };
}
