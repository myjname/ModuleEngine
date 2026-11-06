//! Ten original authored boards and replayable witness solutions.
//! `#` wall, space floor, `.` goal, `$` crate, `@` pawn, `*` crate on goal, `+` pawn on goal.

#[derive(Clone, Copy, Debug)]
pub struct LevelDefinition {
    pub id: &'static str,
    pub title: &'static str,
    pub rows: &'static [&'static str],
    /// A successful move sequence, not a claim of optimality.
    pub solution: &'static str,
}

pub static LEVELS: &[LevelDefinition] = &[
    LevelDefinition {
        id: "level01",
        title: "First Seed",
        rows: &["#######", "#  .  #", "#  $  #", "#  @  #", "#######"],
        solution: "N",
    },
    LevelDefinition {
        id: "level02",
        title: "Twin Shoots",
        rows: &["########", "# .  . #", "# $  $ #", "#  @   #", "########"],
        solution: "WNEESEN",
    },
    LevelDefinition {
        id: "level03",
        title: "Around the Stone",
        rows: &[
            "########", "#.  .  #", "#  #   #", "# $  $ #", "#    @ #", "########",
        ],
        solution: "WWNWEENEESWSWWWWNEESENWWWNESEEN",
    },
    LevelDefinition {
        id: "level04",
        title: "Narrow Passage",
        rows: &[
            "#########",
            "#       #",
            "#  #  $ #",
            "#  #    #",
            "#  $@   #",
            "#.  .   #",
            "#########",
        ],
        solution: "EEENNWSWWSWEEENNWSWSWWNNNEESNWWWSSSEEEENNWSS",
    },
    LevelDefinition {
        id: "level05",
        title: "Willow Walk",
        rows: &[
            "###########",
            "#         #",
            "#   #   $ #",
            "# $ #  #  #",
            "#.@$   #  #",
            "#    .  . #",
            "#         #",
            "###########",
        ],
        solution: "WNNEENEEEEESWWWNWWSSWSENNNEEEEESSNWWWNWWWWSSEESESEEEEENNWSNNWWWSS",
    },
    LevelDefinition {
        id: "level06",
        title: "Three Paths",
        rows: &[
            "##########",
            "#   .    #",
            "# $#  #. #",
            "# @   $  #",
            "#  # $#  #",
            "#        #",
            "#       .#",
            "##########",
        ],
        solution: "NSEEEEWSWNWWWNNESSEESSSEEENNNWWWSSEWWWNNNNEWSSEEESSEENES",
    },
    LevelDefinition {
        id: "level07",
        title: "Orchard Bend",
        rows: &[
            "#########",
            "#       #",
            "# $ #   #",
            "#  $ $  #",
            "#  @#  .#",
            "#   ..  #",
            "#########",
        ],
        solution: "WWNNESEWNNEEESSNNWWSNEESSSNNNWWSSEWSNEEWWWSSENNEEENES",
    },
    LevelDefinition {
        id: "level08",
        title: "Divided Beds",
        rows: &[
            "##########",
            "# .      #",
            "#  ##  $@#",
            "#      $ #",
            "#  $ ##  #",
            "# .      #",
            "#       .#",
            "##########",
        ],
        solution: "WNWWSSEEWWWSWENEEENWEESWWWWWWSEENENEEESSWNWNNWEEESSSSWNNWWNNWW",
    },
    LevelDefinition {
        id: "level09",
        title: "Four Corners",
        rows: &[
            "###########",
            "#. . .$+  #",
            "#  ##     #",
            "# $     $ #",
            "#  $  ##  #",
            "#         #",
            "#         #",
            "###########",
        ],
        solution: "WSWSWWWEEESSEEENNWWWWSWEENEEEENWSWWWWWSSWNEEENEENWWSWWSWNNWNESSSWNN",
    },
    LevelDefinition {
        id: "level10",
        title: "Garden Master",
        rows: &[
            "############",
            "# @      . #",
            "# $# $ #   #",
            "#      $   #",
            "#    ##.   #",
            "#          #",
            "#  # $.#   #",
            "# .        #",
            "############",
        ],
        solution: "SSEEEEESEENNWSWWWWWWSWSENNEEEEEESWSEWWWWWEEEENEESWWWWWNWWSEESSEEEEENNSSWWWWWWWNWNNNEEEEEEEESNWWWWWSWWSSNEEEEENEESWWWWWWWSWSENNEESSEEEEENSWWWWWWEEEEENENWWWWWWWSWSENNEEEEEEENNSSWWWWWWWSSEWNNNNEENENEEEESSWWWNSESSWSSWWWENNNNNNEESSWWSSSENEEEENSWWWWWWWSWSENNNNNNEEEEEWSSEENEESWSSSSWWWWWWENNNNEEEEWWNNEESESESWW",
    },
];

pub fn level_catalog() -> &'static [LevelDefinition] {
    LEVELS
}
pub fn level_solution(id: &str) -> Option<&'static str> {
    LEVELS
        .iter()
        .find(|level| level.id == id)
        .map(|level| level.solution)
}
