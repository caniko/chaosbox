let
  provider = import ./provider.nix;
  answer = 7;
in {
  direct = provider.answer;
  shadowed = let answer = 2; in answer;
  inherited = {inherit answer;};
  dynamic = name: {${name} = answer;};
  text = ''fake = 3;'';
}
