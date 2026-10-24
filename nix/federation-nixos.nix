# Integrated HM owns provider configuration; NixOS owns SSH authorization.
{
  config,
  lib,
  options,
  ...
}:
let
  cfg = config.services.chaosbox.federation;
  homes = lib.attrByPath [ "home-manager" "users" ] { } config;
  providers = lib.filterAttrs (_: home: home.programs.chaosbox.federation.enable or false) homes;
in
{
  options.services.chaosbox.federation.enable =
    lib.mkEnableOption "installing integrated Home Manager Chaosbox query-only SSH endpoints";
  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = lib.hasAttrByPath [ "home-manager" "users" ] options;
        message = "Chaosbox federation SSH endpoints require integrated Home Manager users.";
      }
    ];
    users.users = lib.mapAttrs (_: home: {
      openssh.authorizedKeys.keys = home.programs.chaosbox.federation.authorizedKeyEntries;
    }) providers;
  };
}
