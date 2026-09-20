// Compile vocabulary sections into rows indexed by part of speech.
// The split depends only on the declaration and needs no runtime work.

enum token tokWord;
dictionary cmdDict;
property noun, adjective;
relation vocab(entity: Entity, word: Text, part: Vocabulary) many_to_many;

class Thing: object name = nil;

// Short name; adjectives; nouns. In the short name the last word is the noun
// and the rest are adjectives, so this is a desk, not an oak.
desk: Thing vocab = 'oak desk; large wooden; table counter';
// One word is a noun, and a leading article is a flag rather than a word.
water: Thing vocab = 'some water';
// A declared name is kept: the short name only supplies one that is missing.
rug: Thing name = 'hearth rug' vocab = 'the old rug; threadbare';

show(what, label, words)
{
    "<<what.name>> <<label>>:";
    foreach (local word in words)
        " <<word>>";
    "\n";
    return nil;
}

startup()
{
    show(desk, 'nouns', vocab.all(desk, &noun));
    show(desk, 'adjectives', vocab.all(desk, &adjective));
    show(water, 'nouns', vocab.all(water, &noun));
    show(rug, 'nouns', vocab.all(rug, &noun));
    show(rug, 'adjectives', vocab.all(rug, &adjective));
    "names: <<desk.name>> / <<water.name>> / <<rug.name>>\n";
    // Filed by part of speech, so a noun is not also an adjective.
    "'table' as adjective: <<vocab.contains(desk, 'table', &adjective) ? 'yes' : 'no'>>\n";
    return nil;
}

turn(toks)
{
    return nil;
}
