{
  inputs = {
    naersk.url = "github:nix-community/naersk/master";
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, utils, naersk }:
    utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        naersk-lib = pkgs.callPackage naersk { };
      in
        {
          defaultPackage = naersk-lib.buildPackage {
            src = ./.;

            singleStep = true;
            nativeBuildInputs = with pkgs; [ pkg-config ];
            buildInputs = with pkgs; [ openssl gmp libmpc mpfr
                                       libffi
                                       gnum4];
            preConfigure = ''
    mkdir /tmp/deps
    rsync -aL $crate_sources/ /tmp/deps
    chmod 777 -R /tmp/deps
    crate_sources=/tmp/deps
  '';

            preBuild = ''
    echo '[source]' > $CARGO_HOME/config
    echo '"crates-io" = { "replace-with" = "nix-sources" }' >> $CARGO_HOME/config
    echo '"nix-sources" = { directory = "/tmp/deps" }' >> $CARGO_HOME/config
  '';
          };
          devShell = with pkgs; mkShell {
            buildInputs = [ cargo rustc rustfmt pre-commit rustPackages.clippy

                          ];


            RUST_SRC_PATH = rustPlatform.rustLibSrc;
          };
        });
}
