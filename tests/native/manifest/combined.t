enum token tokWord;
enum Direction: north, south, east, west, up, down;
property firstTokenIndex, lastTokenIndex, tokenList;
dictionary cmdDict;
relation contains(container: Entity, item: Entity) one_to_many reverse location;
relation exits(from: Entity, to: Entity, via: Direction) one_to_one;
relation vocab(entity: Entity, word: Text) many_to_many;
class Thing: object name = 'thing';
class Sensor: Thing reading = 0;
class Station: Thing;
station: Station name = 'station';
meter: Sensor name = 'meter' vocab = 'meter; small; gauge' location = station;
modify meter reading = 17;
startup() { return nil; }
turn(tokens) { return nil; } act(verb, subjects) { return nil; } recover(code) { return nil; }
