# Build the deterministic federation fixtures once for guest tests. The live
# federation regression runs hermetically in the `postgres-live` check.
# Fixtures use explicit synthetic decisions, never a live inference service.
{
  craneLib,
  commonArgs,
  cargoArtifacts,
  ...
}:
craneLib.mkCargoDerivation (
  commonArgs
  // {
    pname = "chaosbox-federation-test-tools";
    inherit cargoArtifacts;
    doCheck = false;
    doInstallCargoArtifacts = false;
    buildPhaseCargoCommand = ''
      cargoWithProfile build --locked -p chaosbox --example federation-fixture
    '';
    installPhaseCommand = ''
      cargoWithProfile run --locked -p chaosbox --example federation-fixture -- "$out/fixtures"
    '';
  }
)
