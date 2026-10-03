# Compile the live regression once, then run it inside a server-present guest.
# Fixtures use explicit synthetic decisions, never a live inference service.
{
  pkgs,
  craneLib,
  commonArgs,
  cargoArtifacts,
}:
craneLib.mkCargoDerivation (
  commonArgs
  // {
    pname = "chaosbox-federation-test-tools";
    inherit cargoArtifacts;
    nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.jq ];
    doCheck = false;
    doInstallCargoArtifacts = false;
    buildPhaseCargoCommand = ''
      cargoWithProfile test --locked -p chaosbox --test federation_typedb --no-run --message-format=json > test-messages.json
      cargoWithProfile build --locked -p chaosbox --example federation-fixture
    '';
    installPhaseCommand = ''
      mkdir -p "$out/bin"
      binary=$(jq -ers '
        [.[] | select(.reason == "compiler-artifact" and .target.name == "federation_typedb" and .executable != null) | .executable]
        | if length == 1 then .[0] else error("expected exactly one federation TypeDB regression binary") end
      ' test-messages.json)
      cp "$binary" "$out/bin/federation-typedb"
      cargoWithProfile run --locked -p chaosbox --example federation-fixture -- "$out/fixtures"
    '';
  }
)
