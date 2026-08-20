{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
}:

let
  # Rolling tag updated on every push to the `deepseek` branch by
  # .github/workflows/release-sandbox.yml. The asset URL is stable; the
  # hash below must be refreshed from `nix store prefetch-file` whenever the
  # artifact is re-uploaded.
  version = "0.24.0";
  repo = "zeroqn/dirge";
  tag = "ds-sandbox";
  triple = "x86_64-unknown-linux-gnu";
in
stdenv.mkDerivation {
  pname = "dirge-bin";
  inherit version;

  src = fetchurl {
    url = "https://github.com/${repo}/releases/download/${tag}/dirge-${triple}-sandbox.tar.gz";
    # PLACEHOLDER — replace after the first workflow run:
    #   nix store prefetch-file --hash-type sha256 <url>
    hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
  };

  nativeBuildInputs = lib.optionals stdenv.isLinux [ autoPatchelfHook ];

  buildInputs = lib.optionals stdenv.isLinux [ stdenv.cc.cc.lib ];

  dontBuild = true;
  sourceRoot = ".";

  installPhase = ''
    runHook preInstall

    # The microVM runner must sit alongside dirge (or on PATH): dirge
    # locates it relative to its own executable first.
    install -Dm755 dirge "$out/bin/dirge"
    install -Dm755 dirge-microvm-runner "$out/bin/dirge-microvm-runner"

    runHook postInstall
  '';

  meta = {
    description = "Minimal, fast pure-Rust coding agent with persistent memory (sandbox-microvm build)";
    homepage = "https://github.com/zeroqn/dirge";
    license = lib.licenses.gpl3Only;
    mainProgram = "dirge";
    platforms = [
      "x86_64-linux"
    ];
  };
}
