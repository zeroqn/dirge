{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
  libkrun,
}:

let
  # Rolling tag updated on every push to the `deepseek` branch by
  # .github/workflows/release-sandbox.yml. The asset URL is stable; the
  # hash in `src` is rewritten automatically by the same workflow after
  # each upload.
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
    # Hash of the artifact uploaded by the last ds-sandbox workflow run;
    # rewritten by .github/workflows/release-sandbox.yml after each upload.
    hash = "sha256-HTG7pei6I+TaWIFjpzA+uTMITKvyuW85IpXDqXvhkq4=";
  };

  nativeBuildInputs = lib.optionals stdenv.isLinux [ autoPatchelfHook ];

  # dirge-microvm-runner links -lkrun at build time and needs libkrun.so.1 at
  # runtime; libkrun dlopens libkrunfw.so, which rides along in libkrun's
  # Nix closure. autoPatchelfHook resolves libkrun.so.1 from this input.
  buildInputs = lib.optionals stdenv.isLinux [
    stdenv.cc.cc.lib
    libkrun
  ];

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
