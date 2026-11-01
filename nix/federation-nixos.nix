# Integrated HM owns provider configuration; NixOS owns SSH authorization and
# can declare shared projects once for every participating owner on the host.
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
  owners = lib.unique (
    lib.concatMap (project: lib.attrNames project.repos) (lib.attrValues cfg.projects)
  );
  # Each owner maps the project to its own repo and grants every other participant.
  projectsFor =
    owner:
    lib.mapAttrs (_: project: {
      repo = project.repos.${owner};
      grants = lib.mapAttrs (_: _: {
        mode = lib.mkDefault project.mode;
        revision = lib.mkDefault project.revision;
      }) (lib.removeAttrs project.repos [ owner ]);
    }) (lib.filterAttrs (_: project: project.repos ? ${owner}) cfg.projects);
in
{
  options.services.chaosbox.federation = {
    enable = lib.mkEnableOption "installing integrated Home Manager Chaosbox query-only SSH endpoints";
    projects = lib.mkOption {
      type = lib.types.attrsOf (
        lib.types.submodule {
          options = {
            repos = lib.mkOption {
              type = lib.types.attrsOf lib.types.nonEmptyStr;
              description = "Participating owners (integrated HM users) mapped to their existing repository association.";
            };
            mode = lib.mkOption {
              type = lib.types.enum [
                "selected"
                "all_admitted"
              ];
              default = "selected";
              description = "Default for each directional grant between the declared owners; all_admitted opts the project into reciprocal sharing.";
            };
            revision = lib.mkOption {
              type = lib.types.ints.positive;
              default = 1;
              description = "Positive grant revision, incremented when project sharing changes.";
            };
          };
        }
      );
      default = { };
      description = "Shared project keys rendered into each participant's programs.chaosbox.federation.projects. Selected record ids and directional overrides belong in that home's grants.";
    };
  };
  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        assertions = [
          {
            assertion = lib.hasAttrByPath [ "home-manager" "users" ] options;
            message = "Chaosbox federation SSH endpoints require integrated Home Manager users.";
          }
          {
            assertion = lib.all (project: lib.length (lib.attrNames project.repos) >= 2) (
              lib.attrValues cfg.projects
            );
            message = "Shared Chaosbox projects require at least two owners.";
          }
          {
            assertion = lib.all (owner: providers ? ${owner}) owners;
            message = "Every shared Chaosbox project owner needs programs.chaosbox.federation.enable in its Home Manager configuration.";
          }
        ];
        users.users = lib.mapAttrs (_: home: {
          openssh.authorizedKeys.keys = home.programs.chaosbox.federation.authorizedKeyEntries;
        }) providers;
      }
      (lib.optionalAttrs (lib.hasAttrByPath [ "home-manager" "users" ] options) {
        home-manager.users = lib.genAttrs owners (owner: {
          programs.chaosbox.federation.projects = projectsFor owner;
        });
      })
    ]
  );
}
