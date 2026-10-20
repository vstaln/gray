use std::path::{Path, PathBuf};

use crate::print::now_millis;

use gray_core::{ContentBlock, Message, Role};
use serde::{Deserialize, Serialize};

const SUPPORTED_SESSION_VERSION: u32 = 1;

const SESSION_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub(crate) const RESTART_PARENT_ENV: &str = "GRAY_RESTART_PARENT_PID";
const RESTART_PARENT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn tighten_file_mode(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

const QUALIFIER_WORDS: &[&str] = &[
    "absorptive",
    "abyssal",
    "acyclic",
    "adiabatic",
    "advective",
    "aerobic",
    "affine",
    "algebraic",
    "alluvial",
    "amorphous",
    "amphiphilic",
    "amphoteric",
    "anaerobic",
    "analytic",
    "anhydrous",
    "anionic",
    "anoxic",
    "anticyclonic",
    "aperiodic",
    "aphotic",
    "apolar",
    "aqueous",
    "arid",
    "associative",
    "astronomical",
    "astrophysical",
    "asymmetric",
    "athermal",
    "atomic",
    "auroral",
    "autotrophic",
    "axial",
    "ballistic",
    "baroclinic",
    "barotropic",
    "baryonic",
    "basaltic",
    "benthic",
    "bilinear",
    "binary",
    "boreal",
    "bosonic",
    "canonical",
    "capacitive",
    "catalytic",
    "cationic",
    "caustic",
    "celestial",
    "cellular",
    "chaotic",
    "charged",
    "chemotrophic",
    "chiral",
    "circadian",
    "cislunar",
    "classical",
    "clastic",
    "coherent",
    "colloidal",
    "combinatorial",
    "commutative",
    "compact",
    "compressible",
    "concave",
    "conformal",
    "continuous",
    "convective",
    "convex",
    "correlated",
    "cosmic",
    "countable",
    "coupled",
    "covalent",
    "covariant",
    "cryogenic",
    "crystalline",
    "cubic",
    "cyclic",
    "cyclonic",
    "decoherent",
    "decoupled",
    "diabatic",
    "dielectric",
    "diffractive",
    "dipolar",
    "discrete",
    "disordered",
    "dispersive",
    "electrodynamic",
    "electronic",
    "electrostatic",
    "electroweak",
    "elliptic",
    "emergent",
    "emissive",
    "endothermic",
    "entangled",
    "entropic",
    "eolian",
    "ergodic",
    "evanescent",
    "exoplanetary",
    "exothermic",
    "exotic",
    "exponential",
    "extragalactic",
    "extremophilic",
    "extrinsic",
    "felsic",
    "fermionic",
    "ferric",
    "ferroelectric",
    "finite",
    "fluorescent",
    "fluvial",
    "fractal",
    "fungal",
    "galactic",
    "gamma",
    "genomic",
    "geodesic",
    "geologic",
    "geometric",
    "geostrophic",
    "glacial",
    "gluonic",
    "granitic",
    "granular",
    "gravitic",
    "hadronic",
    "harmonic",
    "helical",
    "heliocentric",
    "heliotropic",
    "hexagonal",
    "holomorphic",
    "hydrogenic",
    "hydrophilic",
    "hydrophobic",
    "hydrostatic",
    "hydrothermal",
    "hygroscopic",
    "hyperbolic",
    "hyperdense",
    "hyperfine",
    "hypersonic",
    "igneous",
    "incandescent",
    "inert",
    "infinite",
    "infinitesimal",
    "infrared",
    "insoluble",
    "integrable",
    "interstellar",
    "intrinsic",
    "invariant",
    "ionic",
    "ionized",
    "iridescent",
    "irreversible",
    "irrotational",
    "isentropic",
    "isobaric",
    "isochoric",
    "isomeric",
    "isothermal",
    "isotropic",
    "itinerant",
    "jovian",
    "katabatic",
    "kinetic",
    "laminar",
    "latent",
    "linear",
    "local",
    "logarithmic",
    "luminescent",
    "lunar",
    "mafic",
    "magmatic",
    "magnetic",
    "magnetized",
    "magnetostatic",
    "magnonic",
    "meridional",
    "mesoscopic",
    "metallic",
    "metamorphic",
    "metastable",
    "microbial",
    "miscible",
    "monsoonal",
    "multipolar",
    "muonic",
    "nacreous",
    "nested",
    "neural",
    "neutral",
    "noble",
    "nonlinear",
    "nonlocal",
    "nonpolar",
    "nuclear",
    "nucleonic",
    "observable",
    "opaque",
    "orbital",
    "orogenic",
    "orographic",
    "orthogonal",
    "parabolic",
    "parasitic",
    "pelagic",
    "periodic",
    "photic",
    "photoelectric",
    "photonic",
    "phototrophic",
    "phototropic",
    "photovoltaic",
    "piezoelectric",
    "planar",
    "plasmonic",
    "plutonic",
    "polar",
    "polarized",
    "polynomial",
    "porous",
    "primordial",
    "prismatic",
    "prograde",
    "protostellar",
    "pyrophoric",
    "quadratic",
    "quantized",
    "quantum",
    "quasiperiodic",
    "quasistatic",
    "radial",
    "radiative",
    "radioactive",
    "rarefied",
    "recursive",
    "refractive",
    "relativistic",
    "resonant",
    "retrograde",
    "reversible",
    "rhombohedral",
    "rotational",
    "scalar",
    "sedimentary",
    "seismic",
    "sidereal",
    "smooth",
    "solar",
    "soluble",
    "sonic",
    "spectral",
    "spherical",
    "squeezed",
    "stellar",
    "stochastic",
    "stormy",
    "stratified",
    "subatomic",
    "superfluid",
    "superionic",
    "superluminal",
    "supersonic",
    "symbiotic",
    "symmetric",
    "symplectic",
    "synaptic",
    "tectonic",
    "telluric",
    "tensor",
    "thermal",
    "thermionic",
    "thermodynamic",
    "thermoelectric",
    "thermonuclear",
    "tidal",
    "topological",
    "toroidal",
    "transient",
    "translucent",
    "transmissive",
    "transonic",
    "trigonal",
    "tropical",
    "turbulent",
    "twisted",
    "ultracold",
    "ultradense",
    "ultrafast",
    "ultramafic",
    "ultraviolet",
    "unitary",
    "unstable",
    "vector",
    "vesicular",
    "viscous",
    "vitreous",
    "volatile",
    "volcanic",
    "vortical",
    "zodiacal",
];

const MIDDLE_WORDS: &[&str] = &[
    "actinium",
    "adduct",
    "aerogel",
    "aerosol",
    "allotrope",
    "aluminum",
    "americium",
    "amplitudon",
    "anion",
    "antihydrogen",
    "antimony",
    "antineutron",
    "antiparticle",
    "antiproton",
    "antiskyrmion",
    "anyon",
    "argon",
    "arsenic",
    "arsenide",
    "arsine",
    "astatine",
    "atom",
    "axigluon",
    "axino",
    "axion",
    "barium",
    "baryon",
    "berkelium",
    "beryllium",
    "biexciton",
    "bimeron",
    "bino",
    "biphonon",
    "bipolaron",
    "bismuth",
    "bohrium",
    "borane",
    "borazine",
    "boron",
    "borophene",
    "boson",
    "branon",
    "breather",
    "bromine",
    "cadmium",
    "calcium",
    "californium",
    "caloron",
    "carbene",
    "carbide",
    "carbon",
    "carbyne",
    "cation",
    "cerium",
    "cesium",
    "chargino",
    "chelate",
    "chloride",
    "chlorine",
    "chromium",
    "cobalt",
    "colloid",
    "compacton",
    "cooperon",
    "copernicium",
    "copolymer",
    "copper",
    "crowdion",
    "curium",
    "curvaton",
    "darmstadtium",
    "deuteron",
    "diastereomer",
    "dibaryon",
    "dilatino",
    "dilaton",
    "dimer",
    "diquark",
    "doublon",
    "dubnium",
    "dyon",
    "dysprosium",
    "einsteinium",
    "electrolyte",
    "electromagnon",
    "electron",
    "emulsion",
    "enantiomer",
    "erbium",
    "europium",
    "excimer",
    "exciplex",
    "fermion",
    "fermium",
    "ferrite",
    "flerovium",
    "fluoride",
    "fluorine",
    "fluxoid",
    "fluxon",
    "fracton",
    "francium",
    "fullerene",
    "gadolinium",
    "gallium",
    "gaugino",
    "geon",
    "germanene",
    "germanium",
    "glueball",
    "gluino",
    "gluon",
    "gold",
    "goldstino",
    "graphene",
    "graviphoton",
    "graviscalar",
    "gravitino",
    "graviton",
    "hadron",
    "hafnium",
    "hassium",
    "helicon",
    "helimagnon",
    "helion",
    "helium",
    "hexaquark",
    "higgs",
    "higgsino",
    "holmium",
    "holon",
    "hopfion",
    "hydride",
    "hydrogen",
    "hyperon",
    "indium",
    "inflaton",
    "instanton",
    "iodine",
    "iridium",
    "iron",
    "isobar",
    "isomer",
    "isotone",
    "isotope",
    "kaon",
    "krypton",
    "lambda",
    "lanthanum",
    "lawrencium",
    "lead",
    "lepton",
    "leptoquark",
    "leviton",
    "ligand",
    "lithium",
    "livermorium",
    "lutetium",
    "luxon",
    "magnesium",
    "magnon",
    "majorana",
    "majoron",
    "manganese",
    "maxon",
    "meitnerium",
    "mendelevium",
    "mercury",
    "meron",
    "meson",
    "micelle",
    "molecule",
    "molybdenum",
    "monomer",
    "monopole",
    "moscovium",
    "muon",
    "muonium",
    "nanotube",
    "nematicon",
    "neodymium",
    "neon",
    "neptunium",
    "neutralino",
    "neutrino",
    "neutron",
    "nickel",
    "nihonium",
    "niobium",
    "nitrene",
    "nitride",
    "nitrogen",
    "nobelium",
    "nucleon",
    "nuclide",
    "oganesson",
    "oligomer",
    "omega",
    "orbiton",
    "oscillon",
    "osmium",
    "oxide",
    "oxygen",
    "palladium",
    "parafermion",
    "parton",
    "peakon",
    "pentaquark",
    "perovskite",
    "phason",
    "phosphide",
    "phosphine",
    "phosphorene",
    "phosphorus",
    "photino",
    "photon",
    "pion",
    "pionium",
    "plasma",
    "platinum",
    "plutonium",
    "polariton",
    "polaron",
    "polonium",
    "polymer",
    "polymorph",
    "positron",
    "positronium",
    "potassium",
    "praseodymium",
    "preon",
    "promethium",
    "protactinium",
    "proton",
    "protonium",
    "quark",
    "radion",
    "radium",
    "radon",
    "rhenium",
    "rhodium",
    "ripplon",
    "rishon",
    "roentgenium",
    "rotamer",
    "roton",
    "rubidium",
    "ruthenium",
    "rutherfordium",
    "samarium",
    "saxion",
    "scandium",
    "seaborgium",
    "selectron",
    "selenide",
    "selenium",
    "semion",
    "sfermion",
    "sigma",
    "silane",
    "silicene",
    "silicon",
    "silver",
    "skyrmion",
    "skyrmionium",
    "slepton",
    "smuon",
    "sneutrino",
    "sodium",
    "solute",
    "solvent",
    "sphaleron",
    "spinon",
    "squark",
    "stanene",
    "strangelet",
    "strontium",
    "sulfide",
    "sulfur",
    "tachyon",
    "tantalum",
    "tardyon",
    "tautomer",
    "technetium",
    "telluride",
    "tellurium",
    "tennessine",
    "terbium",
    "tetraquark",
    "thallium",
    "thorium",
    "thulium",
    "tin",
    "titanium",
    "toron",
    "trimer",
    "trion",
    "triplon",
    "triton",
    "tungsten",
    "upsilon",
    "uranium",
    "vanadium",
    "vorton",
    "wimp",
    "wimpzilla",
    "wino",
    "xenon",
    "ytterbium",
    "yttrium",
    "zeolite",
    "zinc",
    "zino",
    "zirconium",
    "zwitterion",
];

const PHENOMENON_WORDS: &[&str] = &[
    "ablation",
    "accretion",
    "afterglow",
    "analemma",
    "angstrom",
    "anticline",
    "antinode",
    "aphelion",
    "apogee",
    "arete",
    "asteroid",
    "asymptote",
    "attractor",
    "aurora",
    "barchan",
    "barn",
    "batholith",
    "bergschrund",
    "bifurcation",
    "biome",
    "blazar",
    "blizzard",
    "blueshift",
    "bolide",
    "braid",
    "breakdown",
    "bundle",
    "butte",
    "calcite",
    "caldera",
    "canyon",
    "capacitor",
    "cascade",
    "cavern",
    "cavity",
    "cirque",
    "cirrus",
    "cluster",
    "cohomology",
    "collapsar",
    "collider",
    "comet",
    "condensate",
    "constellation",
    "convection",
    "cordillera",
    "corona",
    "cosmos",
    "coulomb",
    "craton",
    "crevasse",
    "criticality",
    "cryostat",
    "cumulonimbus",
    "cyclone",
    "cyclotron",
    "delta",
    "derecho",
    "diapir",
    "diatreme",
    "diffraction",
    "diffusion",
    "dissipation",
    "domain",
    "drumlin",
    "duality",
    "dune",
    "dustdevil",
    "eclipse",
    "ecliptic",
    "eclogite",
    "ecotone",
    "eigenmode",
    "electronvolt",
    "entropy",
    "epicenter",
    "equinox",
    "ergosphere",
    "escarpment",
    "esker",
    "estuary",
    "exciton",
    "exomoon",
    "exoplanet",
    "farad",
    "fermi",
    "fibration",
    "filament",
    "fjord",
    "flare",
    "fluorescence",
    "flux",
    "foliation",
    "fulgurite",
    "fumarole",
    "galaxy",
    "garnet",
    "gauss",
    "gegenschein",
    "gerbe",
    "geyser",
    "glacier",
    "globule",
    "glory",
    "gneiss",
    "gorge",
    "graupel",
    "gyre",
    "haboob",
    "halo",
    "helictite",
    "heliopause",
    "heliosphere",
    "helix",
    "henry",
    "hertz",
    "homology",
    "homotopy",
    "hoodoo",
    "horizon",
    "hotspot",
    "hypernova",
    "hysteresis",
    "iceberg",
    "ignimbrite",
    "infinity",
    "inselberg",
    "ionosphere",
    "isomorph",
    "isotopy",
    "jetstream",
    "joule",
    "karst",
    "kelvin",
    "kilonova",
    "klystron",
    "knot",
    "laccolith",
    "lagoon",
    "lahar",
    "laser",
    "lattice",
    "lenticular",
    "libration",
    "locus",
    "lumen",
    "maar",
    "maelstrom",
    "magnetar",
    "magnetite",
    "magnetosphere",
    "magnetotail",
    "magnetron",
    "mammatus",
    "manifold",
    "maser",
    "massif",
    "meander",
    "meridian",
    "mesa",
    "mesocyclone",
    "meteor",
    "meteorite",
    "mirage",
    "monadnock",
    "monsoon",
    "moonbow",
    "moonmilk",
    "moraine",
    "morphism",
    "multiverse",
    "nadir",
    "nebula",
    "nimbus",
    "nova",
    "nucleation",
    "nunatak",
    "nutation",
    "oasis",
    "obsidian",
    "occultation",
    "olivine",
    "orbifold",
    "orbit",
    "orogeny",
    "outcrop",
    "overtone",
    "oxbow",
    "pahoehoe",
    "paradox",
    "parallax",
    "parhelion",
    "parsec",
    "pascal",
    "pegmatite",
    "penumbra",
    "percolation",
    "periapsis",
    "perigee",
    "perihelion",
    "permafrost",
    "phonon",
    "pinch",
    "pingo",
    "planetesimal",
    "plasmasphere",
    "plasmoid",
    "plasmon",
    "playa",
    "plume",
    "polyhedron",
    "polytope",
    "precession",
    "protoplanet",
    "protostar",
    "pulsar",
    "pumice",
    "pyrite",
    "quartz",
    "quartzite",
    "quasar",
    "quasicrystal",
    "rainbow",
    "reconnection",
    "redshift",
    "refraction",
    "regolith",
    "resonance",
    "sandstorm",
    "sastrugi",
    "savanna",
    "seamount",
    "separatrix",
    "sheaf",
    "sheath",
    "simplex",
    "singularity",
    "solenoid",
    "soliton",
    "solstice",
    "spectrum",
    "speleothem",
    "spheromak",
    "spicule",
    "spiral",
    "spline",
    "sprite",
    "stalactite",
    "starburst",
    "stellarator",
    "stratovolcano",
    "subduction",
    "sublimation",
    "sundog",
    "sunspot",
    "supercell",
    "supercluster",
    "supergiant",
    "supernova",
    "synchrotron",
    "syncline",
    "syzygy",
    "talus",
    "tarn",
    "telescope",
    "tempest",
    "terminator",
    "terrane",
    "tesla",
    "tessellation",
    "tesseract",
    "tetrahedron",
    "threshold",
    "thunderstorm",
    "tokamak",
    "topology",
    "topos",
    "tornado",
    "toroid",
    "torus",
    "transit",
    "trefoil",
    "trojan",
    "tsunami",
    "tundra",
    "umbra",
    "universe",
    "vertex",
    "virga",
    "void",
    "volcano",
    "volt",
    "vortex",
    "waterfall",
    "wavefront",
    "waveguide",
    "weber",
    "whirlpool",
    "williwaw",
    "wormhole",
    "xenolith",
    "yardang",
    "zenith",
    "zircon",
];

const FRESH_ID_ATTEMPTS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    pub fn generate() -> Self {
        let bits = uuid::Uuid::new_v4().as_u128();
        let q = QUALIFIER_WORDS[(bits as usize) % QUALIFIER_WORDS.len()];
        let m = MIDDLE_WORDS[((bits >> 32) as usize) % MIDDLE_WORDS.len()];
        let p = PHENOMENON_WORDS[((bits >> 64) as usize) % PHENOMENON_WORDS.len()];
        Self(format!("{q}-{m}-{p}"))
    }

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub type SessionEntryId = u64;

type CarriedMeta = (u64, Option<gray_core::event::Usage>, Option<u64>);

/// What survives a history rewrite: a tool round by its call id (soft
/// elision rewrites the result body, not the id), anything else by content.
fn carry_key(m: &Message) -> Option<String> {
    let id = m.content.iter().find_map(|b| match b {
        ContentBlock::ToolUse { id, .. } | ContentBlock::ToolResult { id, .. } => Some(id),
        _ => None,
    });
    match id {
        Some(id) => Some(format!("{:?}:{id}", m.role)),
        None => serde_json::to_string(m).ok(),
    }
}

/// Timestamp/usage/duration of the active entries (after the last
/// boundary), so a replacement re-states kept messages as they were rather
/// than stamping the whole transcript with the rewrite time.
fn carried_meta(
    text: &str,
) -> std::collections::HashMap<String, std::collections::VecDeque<CarriedMeta>> {
    let entries: Vec<SessionEntry> = text
        .lines()
        .skip(1)
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let start = entries
        .iter()
        .rposition(|e| e.compaction_boundary)
        .map_or(0, |i| i + 1);
    let mut map: std::collections::HashMap<_, std::collections::VecDeque<_>> = Default::default();
    for e in &entries[start..] {
        if let Some(k) = carry_key(&e.message) {
            map.entry(k)
                .or_default()
                .push_back((e.timestamp, e.usage, e.duration_ms));
        }
    }
    map
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: SessionId,

    pub timestamp: u64,

    pub cwd: PathBuf,

    pub model: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

impl SessionMeta {
    pub fn new(
        id: SessionId,
        timestamp: u64,
        cwd: impl Into<PathBuf>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            id,
            timestamp,
            cwd: cwd.into(),
            model: model.into(),
            origin: None,
        }
    }

    pub fn with_origin(mut self, origin: Option<String>) -> Self {
        self.origin = origin;
        self
    }
}

fn sanitize_origin_tag(raw: &str) -> Option<String> {
    let t = raw.trim().to_ascii_lowercase();
    (!t.is_empty()
        && t.len() <= 32
        && t.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'))
    .then_some(t)
}

static INHERITED_SESSION: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
static RESTART_PARENT_PID: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();

pub fn capture_inherited_session_env() {
    INHERITED_SESSION.get_or_init(|| {
        std::env::var("GRAY_SESSION_ID")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    });
    RESTART_PARENT_PID.get_or_init(|| {
        std::env::var(RESTART_PARENT_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|&p| p != std::process::id())
    });
}

pub fn session_origin_from_env() -> Option<String> {
    if let Some(v) = std::env::var("GRAY_SESSION_ORIGIN")
        .ok()
        .and_then(|s| sanitize_origin_tag(&s))
    {
        return Some(v);
    }
    if std::env::var("GRAY_SUBAGENTS_ACTIVE").as_deref() == Ok("1") {
        return Some("subagent".to_string());
    }
    if INHERITED_SESSION.get().and_then(Option::as_deref).is_some() {
        return Some("nested".to_string());
    }
    None
}

pub fn print_session_origin() -> Option<String> {
    session_origin_from_env().or_else(|| Some("print".to_string()))
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEntry {
    #[serde(default, skip_serializing_if = "is_false")]
    pub compaction_boundary: bool,

    pub entry_id: u64,

    pub parent_id: Option<u64>,

    pub timestamp: u64,

    pub message: Message,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<gray_core::event::Usage>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEnd {
    #[default]
    Done,

    ToolUse,

    ToolResult,

    UserUnanswered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    #[serde(default)]
    pub last_end: SessionEnd,

    #[serde(default)]
    pub dismissed_at: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    pub id: SessionId,

    pub started_at: u64,

    pub cwd: PathBuf,

    pub first_user_text: Option<String>,

    #[serde(default)]
    pub last_user_text: Option<String>,

    #[serde(default)]
    pub last_message_at: u64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("session {0} not found")]
    NotFound(SessionId),

    #[error("session {0} already exists")]
    AlreadyExists(SessionId),

    #[error(
        "session '{id}' is already open in another process{}. Close the other instance before opening it here.",
        .pid.map(|p| format!(" (PID {p})")).unwrap_or_default()
    )]
    Locked { id: SessionId, pid: Option<u32> },

    #[error("corrupt entry at {}:{}: {source}", path.display(), line)]
    Corrupt {
        path: PathBuf,

        line: usize,

        #[source]
        source: serde_json::Error,
    },
}

pub type Result<T> = std::result::Result<T, SessionError>;

impl SessionError {
    pub fn locked_notice(id: &SessionId, pid: Option<u32>) -> String {
        let head = match pid {
            Some(p) => format!("session '{id}' is already open in another process (pid {p})"),
            None => format!("session '{id}' is already open in another process"),
        };
        format!("{head}\nclose the other instance, or open a different session")
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Header {
    version: u32,
    id: SessionId,
    timestamp: u64,
    cwd: PathBuf,
    model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
}

pub struct JsonlSessionStore {
    root_dir: PathBuf,
    lock: tokio::sync::Mutex<()>,
}

impl Default for JsonlSessionStore {
    fn default() -> Self {
        Self::new(default_root().unwrap_or_else(|| PathBuf::from(".gray/sessions")))
    }
}

pub(crate) fn valid_session_id(s: &str) -> bool {
    if s.is_empty()
        || s.len() > 128
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    if matches!(lower.as_str(), "con" | "prn" | "aux" | "nul") {
        return false;
    }
    if lower.len() == 4
        && (lower.starts_with("com") || lower.starts_with("lpt"))
        && lower.as_bytes()[3].is_ascii_digit()
        && lower.as_bytes()[3] != b'0'
    {
        return false;
    }
    true
}

impl JsonlSessionStore {
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root_dir.into(),
            lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    pub(crate) fn resolve_unique_prefix(&self, raw: &str) -> std::result::Result<String, String> {
        if !valid_session_id(raw) {
            return Err(format!("invalid session id {raw:?}"));
        }
        if self.root_dir.join(format!("{raw}.jsonl")).is_file() {
            return Ok(raw.to_string());
        }
        let mut matches: Vec<String> = match std::fs::read_dir(&self.root_dir) {
            Ok(read) => read
                .flatten()
                .filter_map(|e| {
                    e.file_name()
                        .into_string()
                        .ok()
                        .and_then(|n| n.strip_suffix(".jsonl").map(str::to_owned))
                })
                .filter(|stem| stem.starts_with(raw))
                .collect(),
            Err(_) => Vec::new(),
        };
        matches.sort();
        matches.dedup();
        match matches.len() {
            0 => Err(format!("no session {raw:?} in {}", self.root_dir.display())),
            1 => Ok(matches.remove(0)),
            _ => {
                let shown = matches
                    .iter()
                    .take(8)
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                let more = matches.len().saturating_sub(8);
                Err(format!(
                    "ambiguous session id {raw:?} — matches: {shown}{}",
                    if more > 0 {
                        format!(" (+{more} more)")
                    } else {
                        String::new()
                    }
                ))
            }
        }
    }

    fn fnv1a_hex(s: &str) -> String {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in s.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{h:016x}")
    }

    fn remembered_dir(&self) -> PathBuf {
        self.root_dir.join("remembered")
    }

    fn remember_path_for_cwd(&self, cwd: &Path) -> PathBuf {
        let key = cwd
            .canonicalize()
            .unwrap_or_else(|_| cwd.to_path_buf())
            .display()
            .to_string();
        self.remembered_dir()
            .join(format!("{}.txt", Self::fnv1a_hex(&key)))
    }

    pub async fn remember(&self, id: &SessionId, cwd: &Path) {
        if !valid_session_id(id.as_str()) {
            return;
        }
        let dir = self.remembered_dir();
        if ensure_private_dir(&dir).is_err() {
            return;
        }
        let path = self.remember_path_for_cwd(cwd);
        let stored_cwd = cwd
            .canonicalize()
            .unwrap_or_else(|_| cwd.to_path_buf())
            .display()
            .to_string();
        let tmp = dir.join(format!(".tmp-{}", Self::fnv1a_hex(id.as_str())));
        let content = format!("{stored_cwd}\n{}\n", id.as_str());
        if tokio::fs::write(&tmp, content.as_bytes()).await.is_err() {
            return;
        }
        tighten_file_mode(&tmp);
        if tokio::fs::rename(&tmp, &path).await.is_err() {
            let _ = tokio::fs::remove_file(&tmp).await;
            return;
        }
        tighten_file_mode(&path);
    }

    pub async fn recall(&self, cwd: &Path) -> Option<SessionId> {
        let content = tokio::fs::read_to_string(self.remember_path_for_cwd(cwd))
            .await
            .ok()?;
        let mut lines = content.lines();
        let stored_cwd = PathBuf::from(lines.next()?.trim());
        let current = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        let stored = stored_cwd
            .canonicalize()
            .unwrap_or_else(|_| stored_cwd.clone());
        if stored != current && stored_cwd != current {
            return None;
        }
        let id_str = lines.next()?.trim();
        if !valid_session_id(id_str) {
            return None;
        }
        Some(SessionId::new(id_str))
    }

    pub async fn recall_validated(&self, cwd: &Path) -> Option<SessionId> {
        let id = self.recall(cwd).await?;
        let path = self.session_path(&id).ok()?;
        match tokio::fs::metadata(&path).await {
            Ok(m) => {
                if m.is_file() {
                    Some(id)
                } else {
                    None
                }
            }
            Err(_) => None,
        }
    }

    pub async fn forget(&self, cwd: &Path) {
        let _ = tokio::fs::remove_file(self.remember_path_for_cwd(cwd)).await;
    }

    pub(crate) async fn exists_on_disk(&self, id: &SessionId) -> bool {
        let Ok(path) = self.session_path(id) else {
            return false;
        };
        tokio::fs::metadata(&path)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false)
    }

    pub async fn fresh_id(&self) -> SessionId {
        self.fresh_id_with(SessionId::generate).await
    }

    async fn fresh_id_with(&self, mut next_id: impl FnMut() -> SessionId) -> SessionId {
        for _ in 0..FRESH_ID_ATTEMPTS {
            let id = next_id();
            if !self.exists_on_disk(&id).await {
                return id;
            }
        }
        let base = next_id();
        let salt = (uuid::Uuid::new_v4().as_u128() & 0xffff) as u16;
        SessionId::new(format!("{}-{salt:04x}", base.as_str()))
    }

    async fn next_ids_for_append(id: &SessionId, path: &Path) -> Result<(u64, Option<u64>)> {
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(SessionError::NotFound(id.clone()));
            }
            Err(e) => return Err(SessionError::Io(e)),
        };

        if !content.ends_with('\n')
            && let Some(cut) = content.rfind('\n')
        {
            let file = tokio::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .await
                .map_err(SessionError::Io)?;
            file.set_len((cut + 1) as u64)
                .await
                .map_err(SessionError::Io)?;
            file.sync_all().await.map_err(SessionError::Io)?;
            log::warn!(target: "gray_session",
                "{}:{} truncated torn tail to last complete record",
                path.display(), cut + 1);
            return Self::scan_entries(id, &content[..cut + 1], path);
        }
        Self::scan_entries(id, &content, path)
    }

    fn session_path(&self, id: &SessionId) -> std::io::Result<PathBuf> {
        let s = id.as_str();
        if !valid_session_id(s) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid session identifier",
            ));
        }
        Ok(self.root_dir.join(format!("{s}.jsonl")))
    }

    fn session_lock_path(&self, id: &SessionId) -> std::io::Result<PathBuf> {
        self.session_path(id)?;
        Ok(self.root_dir.join(format!("{}.lock", id.as_str())))
    }

    async fn lock_session_file(&self, id: &SessionId) -> Result<std::fs::File> {
        let lock_path = self.session_lock_path(id)?;
        ensure_private_dir(&self.root_dir)?;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(false).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&lock_path)?;
        tighten_file_mode(&lock_path);
        let deadline = std::time::Instant::now() + SESSION_LOCK_TIMEOUT;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(SessionError::Io(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("timed out waiting for session lock {}", lock_path.display()),
                        )));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                Err(std::fs::TryLockError::Error(e)) => {
                    log::warn!(
                        "session file locking unsupported on {} ({e}); proceeding unlocked",
                        lock_path.display()
                    );
                    return Ok(file);
                }
            }
        }
    }

    fn session_open_path(&self, id: &SessionId) -> std::io::Result<PathBuf> {
        self.session_path(id)?;
        Ok(self.root_dir.join(format!("{}.open", id.as_str())))
    }

    pub async fn acquire_open(&self, id: &SessionId) -> Result<SessionOpenGuard> {
        let open_path = self.session_open_path(id)?;
        ensure_private_dir(&self.root_dir)?;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&open_path)?;
        tighten_file_mode(&open_path);

        let mut held = OPEN_LOCKS.lock().expect("open-lock registry poisoned");
        if let Some(inner) = held.get(&open_path).and_then(std::sync::Weak::upgrade) {
            return Ok(SessionOpenGuard { _inner: inner });
        }
        held.remove(&open_path);
        drop(held);

        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                if !wait_out_restart_parent(&mut file, &open_path).await {
                    return Err(SessionError::Locked {
                        id: id.clone(),
                        pid: read_open_holder(&open_path),
                    });
                }
            }
            Err(std::fs::TryLockError::Error(e)) => {
                log::warn!(
                    "session open-lock unsupported on {} ({e}); proceeding unlocked",
                    open_path.display()
                );
            }
        }
        write_open_holder(&mut file, &open_path);
        let inner = std::sync::Arc::new(OpenLockInner { _file: file });
        OPEN_LOCKS
            .lock()
            .expect("open-lock registry poisoned")
            .insert(open_path, std::sync::Arc::downgrade(&inner));
        Ok(SessionOpenGuard { _inner: inner })
    }

    pub async fn open_lock_held(&self, id: &SessionId) -> bool {
        self.open_lock_held_sync(id)
    }

    pub(crate) fn open_lock_held_sync(&self, id: &SessionId) -> bool {
        let Ok(open_path) = self.session_open_path(id) else {
            return false;
        };
        open_lock_held_at(&open_path)
    }

    pub async fn held_open_locks(&self, ids: Vec<SessionId>) -> std::collections::HashSet<String> {
        use futures::StreamExt as _;
        let mut probes = futures::stream::iter(
            ids.into_iter()
                .filter_map(|id| self.session_open_path(&id).ok()),
        )
        .map(|path| {
            tokio::task::spawn_blocking(move || {
                if !open_lock_held_at(&path) {
                    return None;
                }
                path.file_stem().and_then(|s| s.to_str()).map(str::to_owned)
            })
        })
        .buffer_unordered(32);
        let mut held = std::collections::HashSet::new();
        while let Some(joined) = probes.next().await {
            if let Ok(Some(stem)) = joined {
                held.insert(stem);
            }
        }
        held
    }

    pub(crate) fn open_holder_pid(&self, id: &SessionId) -> Option<u32> {
        read_open_holder(&self.session_open_path(id).ok()?)
    }

    async fn quarantine_corrupt_file(path: &Path) {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            return;
        };
        let Some(parent) = path.parent() else {
            return;
        };
        let prefix = format!("{stem}.corrupt-");
        let mut max_n = 0u32;
        if let Ok(mut rd) = tokio::fs::read_dir(parent).await {
            while let Ok(Some(e)) = rd.next_entry().await {
                let name = e.file_name().to_string_lossy().into_owned();
                if let Some(n) = name.strip_prefix(&prefix).and_then(|s| s.parse().ok()) {
                    max_n = max_n.max(n);
                }
            }
        }
        if tokio::fs::rename(path, parent.join(format!("{prefix}{}", max_n + 1)))
            .await
            .is_err()
        {
            return;
        }
        if max_n + 1 > 3 {
            for n in 1..=(max_n + 1 - 3) {
                let _ = tokio::fs::remove_file(parent.join(format!("{prefix}{n}"))).await;
            }
        }
    }
}

pub(crate) enum SessionScanEvent {
    Running(std::collections::HashSet<String>),

    Summary(SessionSummary),

    Done,
}

enum SessionFileRead {
    Summary(SessionSummary),

    Corrupt(PathBuf),
}

fn user_summary_text(message: &Message) -> String {
    let text = message.text_content();
    if !text.is_empty() {
        return text;
    }
    let has = |f: fn(&ContentBlock) -> bool| message.content.iter().any(f);
    if has(|b| matches!(b, ContentBlock::Image { .. })) {
        "[Image]".to_string()
    } else if has(|b| matches!(b, ContentBlock::Media { .. })) {
        "[File]".to_string()
    } else {
        String::new()
    }
}

fn summarize_session_file(root: &Path, path: &Path, cwd: Option<&Path>) -> Option<SessionFileRead> {
    let slices = read_session_ends(path, cwd)?;
    let stem = path.file_stem().and_then(|s| s.to_str())?;

    let complete_first_line = slices.head.contains('\n');

    let header_str = match slices.head.lines().find(|l| !l.trim().is_empty()) {
        Some(h) => h,
        None => {
            log::warn!("skipping empty session file: {}", path.display());
            return None;
        }
    };

    let header: Header = match serde_json::from_str(header_str) {
        Ok(h) => h,
        Err(e) => {
            log::warn!("skipping corrupt header in {}: {}", path.display(), e);

            if complete_first_line {
                return Some(SessionFileRead::Corrupt(path.to_path_buf()));
            }
            return None;
        }
    };
    if header.id.as_str() != stem {
        log::warn!(
            "skipping session whose header id does not match its filename: {}",
            path.display()
        );
        return None;
    }

    let mut last_message_at = header.timestamp;
    let mut first_user_text = None;
    let mut last_user_text = None;
    let mut last_end = SessionEnd::Done;

    for (in_tail, chunk) in [(false, &slices.head), (true, &slices.tail)] {
        for line in chunk.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(entry) = serde_json::from_str::<SessionEntry>(line) else {
                continue;
            };
            last_message_at = last_message_at.max(entry.timestamp);
            if entry.compaction_boundary {
                first_user_text = None;
                last_user_text = None;
                continue;
            }
            let blocks = &entry.message.content;
            let has_block = |f: fn(&ContentBlock) -> bool| blocks.iter().any(f);
            last_end = match entry.message.role {
                Role::Assistant if has_block(|b| matches!(b, ContentBlock::ToolUse { .. })) => {
                    SessionEnd::ToolUse
                }
                Role::Assistant => SessionEnd::Done,
                Role::User if has_block(|b| matches!(b, ContentBlock::ToolResult { .. })) => {
                    SessionEnd::ToolResult
                }
                Role::User => SessionEnd::UserUnanswered,
                Role::System => last_end,
            };
            if entry.message.role == Role::User {
                let text = user_summary_text(&entry.message);
                if !text.is_empty() {
                    if first_user_text.is_none() && !in_tail {
                        first_user_text = Some(text.clone());
                    }
                    last_user_text = Some(text);
                }
            }
        }
    }

    Some(SessionFileRead::Summary(SessionSummary {
        id: header.id,
        started_at: header.timestamp,
        last_message_at,
        cwd: header.cwd,
        first_user_text,
        last_user_text,
        last_end,
        dismissed_at: if last_end == SessionEnd::Done {
            None
        } else {
            read_dismissal(root, stem)
        },
        title: read_title(root, stem),
        origin: header.origin,
    }))
}

async fn scan_session_dir(
    root: PathBuf,
    cwd: Option<PathBuf>,
    probe_locks: bool,
    tx: tokio::sync::mpsc::UnboundedSender<SessionScanEvent>,
) {
    let mut read_dir = match tokio::fs::read_dir(&root).await {
        Ok(rd) => rd,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("failed to read session directory {}: {}", root.display(), e);
            }
            let _ = tx.send(SessionScanEvent::Done);
            return;
        }
    };

    let mut paths = Vec::new();
    let mut open_paths = Vec::new();
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let path = entry.path();
        let ext = path.extension().and_then(|ext| ext.to_str());
        if ext == Some("open") {
            if probe_locks
                && path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(valid_session_id)
            {
                open_paths.push(path);
            }
            continue;
        }
        if ext != Some("jsonl") {
            continue;
        }

        if !entry
            .file_type()
            .await
            .map(|t| t.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !valid_session_id(stem) {
            log::warn!("skipping session with invalid id: {}", path.display());
            continue;
        }
        paths.push(path);
    }

    if probe_locks {
        let probe_tx = tx.clone();
        tokio::spawn(async move {
            use futures::StreamExt as _;
            let mut probes = futures::stream::iter(open_paths)
                .map(|path| {
                    tokio::task::spawn_blocking(move || {
                        if !open_lock_held_at(&path) {
                            return None;
                        }
                        path.file_stem().and_then(|s| s.to_str()).map(str::to_owned)
                    })
                })
                .buffer_unordered(32);
            let mut held = std::collections::HashSet::new();
            while let Some(joined) = probes.next().await {
                if let Ok(Some(stem)) = joined {
                    held.insert(stem);
                }
            }
            let _ = probe_tx.send(SessionScanEvent::Running(held));
        });
    }

    use futures::StreamExt as _;
    let mut reads = futures::stream::iter(paths)
        .map(|path| {
            let root = root.clone();
            let cwd = cwd.clone();
            async move {
                tokio::task::spawn_blocking(move || {
                    summarize_session_file(&root, &path, cwd.as_deref())
                })
                .await
            }
        })
        .buffer_unordered(64);
    while let Some(joined) = reads.next().await {
        let Ok(Some(read)) = joined else {
            continue;
        };
        match read {
            SessionFileRead::Summary(s) => {
                if tx.send(SessionScanEvent::Summary(s)).is_err() {
                    return;
                }
            }
            SessionFileRead::Corrupt(path) => {
                JsonlSessionStore::quarantine_corrupt_file(&path).await;
            }
        }
    }
    let _ = tx.send(SessionScanEvent::Done);
}

const SUMMARY_END_BYTES: u64 = 16 * 1024;

const BOUNDARY_SCAN_BYTES: u64 = 64 * 1024;

struct SessionSlices {
    head: String,
    tail: String,
}

fn read_session_ends(path: &Path, cwd: Option<&Path>) -> Option<SessionSlices> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok().or_else(|| {
        log::warn!("failed to open session file {}", path.display());
        None
    })?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None;
    }

    let head_len = SUMMARY_END_BYTES.min(len);
    let mut head_bytes = vec![0u8; head_len as usize];
    file.read_exact(&mut head_bytes).ok()?;
    if let Some(cwd) = cwd
        && let Some(end) = head_bytes.iter().position(|b| *b == b'\n')
        && let Ok(header) = serde_json::from_slice::<Header>(&head_bytes[..end])
        && header.cwd != cwd
    {
        return None;
    }
    if head_len < len && !head_bytes.last().is_some_and(|b| *b == b'\n') {
        let mut scan = vec![0u8; BOUNDARY_SCAN_BYTES as usize];
        let mut got = 0usize;
        while got < scan.len() {
            let n = file.read(&mut scan[got..]).ok()?;
            if n == 0 {
                break;
            }
            got += n;
            if scan[..got].contains(&b'\n') {
                break;
            }
        }
        match scan[..got].iter().position(|b| *b == b'\n') {
            Some(at) => head_bytes.extend_from_slice(&scan[..=at]),

            None => return read_session_whole(path),
        }
    }
    let head = String::from_utf8_lossy(&head_bytes).into_owned();

    if len <= head_bytes.len() as u64 + SUMMARY_END_BYTES {
        return Some(SessionSlices {
            head,
            tail: String::new(),
        });
    }
    file.seek(SeekFrom::Start(len - SUMMARY_END_BYTES)).ok()?;
    let mut tail_bytes = Vec::with_capacity(SUMMARY_END_BYTES as usize);
    file.read_to_end(&mut tail_bytes).ok()?;
    let text = String::from_utf8_lossy(&tail_bytes).into_owned();
    let tail = match text.find('\n') {
        Some(at) => text[at + 1..].to_string(),
        None => return read_session_whole(path),
    };
    Some(SessionSlices { head, tail })
}

fn read_session_whole(path: &Path) -> Option<SessionSlices> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("failed to read session file {}: {}", path.display(), e);
            return None;
        }
    };
    Some(SessionSlices {
        head: content,
        tail: String::new(),
    })
}

impl JsonlSessionStore {
    pub async fn create(&self, meta: SessionMeta) -> Result<SessionId> {
        let _guard = self.lock.lock().await;
        let id = meta.id.clone();
        let path = self.session_path(&id)?;

        ensure_private_dir(&self.root_dir)?;

        let header = Header {
            version: SUPPORTED_SESSION_VERSION,
            id: meta.id,
            timestamp: meta.timestamp,
            cwd: meta.cwd,
            model: meta.model,
            origin: meta.origin,
        };

        let json = serde_json::to_string(&header)?;
        let line = format!("{json}\n");

        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        match options.open(&path).await {
            Ok(mut file) => {
                use tokio::io::AsyncWriteExt;
                file.write_all(line.as_bytes()).await?;
                file.flush().await?;
                file.sync_all().await?;
                tighten_file_mode(&path);

                if header.origin.is_none() {
                    self.remember(&id, &header.cwd).await;
                }
                Ok(id)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(SessionError::AlreadyExists(id))
            }
            Err(e) => Err(SessionError::Io(e)),
        }
    }

    fn scan_entries(id: &SessionId, content: &str, path: &Path) -> Result<(u64, Option<u64>)> {
        let mut lines = content.lines().filter(|l| !l.trim().is_empty());
        if lines.next().is_none() {
            return Err(SessionError::NotFound(id.clone()));
        }
        if !content.ends_with('\n') {
            return Err(SessionError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "session has an incomplete tail; repair before appending",
            )));
        }
        let mut max_id: Option<u64> = None;
        let mut last_id: Option<u64> = None;
        for (n, line) in lines.enumerate() {
            let entry = serde_json::from_str::<SessionEntry>(line).map_err(|source| {
                SessionError::Corrupt {
                    path: path.to_path_buf(),

                    line: n + 2,
                    source,
                }
            })?;
            max_id = Some(max_id.map_or(entry.entry_id, |m| m.max(entry.entry_id)));
            last_id = Some(entry.entry_id);
        }
        Ok((max_id.map_or(0, |m| m + 1), last_id))
    }

    pub async fn append(&self, id: &SessionId, msg: &Message) -> Result<SessionEntryId> {
        self.append_with_usage_and_duration(id, msg, None, None)
            .await
    }

    pub async fn append_with_usage_and_duration(
        &self,
        id: &SessionId,
        msg: &Message,
        usage: Option<gray_core::event::Usage>,
        duration_ms: Option<u64>,
    ) -> Result<SessionEntryId> {
        let _lock_file = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;
        let path = self.session_path(id)?;

        let (next_id, parent_id) = Self::next_ids_for_append(id, &path).await?;

        let entry = SessionEntry {
            compaction_boundary: false,
            entry_id: next_id,
            parent_id,
            timestamp: now_millis(),
            message: msg.clone(),
            usage,
            duration_ms,
        };

        let json = serde_json::to_string(&entry)?;
        let line = format!("{}\n", json);

        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .create(false)
            .open(&path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    SessionError::NotFound(id.clone())
                } else {
                    SessionError::Io(e)
                }
            })?;

        file.write_all(line.as_bytes()).await?;
        file.flush().await?;
        file.sync_all().await?;
        tighten_file_mode(&path);

        Ok(next_id)
    }

    pub async fn append_compaction_replacement(
        &self,
        id: &SessionId,
        replacement: &[Message],
    ) -> Result<()> {
        let _lock_file = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;
        let path = self.session_path(id)?;

        let (mut next_id, mut parent_id) = Self::next_ids_for_append(id, &path).await?;
        let mut carried = match Self::read_session_text(&path).await? {
            Some((text, _)) => carried_meta(&text),
            None => Default::default(),
        };

        let mut out = String::new();

        let mut msgs = Vec::with_capacity(replacement.len() + 1);
        msgs.push((
            true,
            Message::system(
                "compaction boundary: entries before this point are superseded; replay starts after it",
            ),
        ));
        for msg in replacement {
            msgs.push((false, msg.clone()));
        }
        for (boundary, msg) in msgs {
            let (timestamp, usage, duration_ms) = carry_key(&msg)
                .and_then(|k| carried.get_mut(&k)?.pop_front())
                .filter(|_| !boundary)
                .unwrap_or((now_millis(), None, None));
            let entry = SessionEntry {
                compaction_boundary: boundary,
                entry_id: next_id,
                parent_id,
                timestamp,
                message: msg,
                usage,
                duration_ms,
            };
            let json = serde_json::to_string(&entry)?;
            out.push_str(&json);
            out.push('\n');
            parent_id = Some(next_id);
            next_id += 1;
        }

        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .create(false)
            .open(&path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    SessionError::NotFound(id.clone())
                } else {
                    SessionError::Io(e)
                }
            })?;

        file.write_all(out.as_bytes()).await?;
        file.flush().await?;
        file.sync_all().await?;
        tighten_file_mode(&path);

        Ok(())
    }

    fn validate_loaded_graph(
        path: &Path,
        expected_id: &SessionId,
        header_line_num: usize,
        header: &Header,
        entries: &[SessionEntry],
        entry_line_nums: &[usize],
    ) -> Result<()> {
        use serde::de::Error as _;
        if header.version != SUPPORTED_SESSION_VERSION {
            return Err(SessionError::Corrupt {
                path: path.to_path_buf(),
                line: header_line_num,
                source: serde_json::Error::custom(format!(
                    "unsupported session version {} (expected {})",
                    header.version, SUPPORTED_SESSION_VERSION
                )),
            });
        }
        if header.id != *expected_id {
            return Err(SessionError::Corrupt {
                path: path.to_path_buf(),
                line: header_line_num,
                source: serde_json::Error::custom(format!(
                    "session id mismatch: filename '{}' != header id '{}'",
                    expected_id.as_str(),
                    header.id.as_str()
                )),
            });
        }
        let mut seen: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        for (i, entry) in entries.iter().enumerate() {
            if let Some(prev) = seen.insert(entry.entry_id, i) {
                let _ = prev;
                return Err(SessionError::Corrupt {
                    path: path.to_path_buf(),
                    line: entry_line_nums.get(i).copied().unwrap_or(0),
                    source: serde_json::Error::custom(format!(
                        "duplicate entry id {}",
                        entry.entry_id
                    )),
                });
            }
        }
        let index_of = |eid: u64| seen.get(&eid).copied();
        for (i, entry) in entries.iter().enumerate() {
            let Some(parent) = entry.parent_id else {
                continue;
            };
            let line = entry_line_nums.get(i).copied().unwrap_or(0);
            if parent == entry.entry_id {
                return Err(SessionError::Corrupt {
                    path: path.to_path_buf(),
                    line,
                    source: serde_json::Error::custom(format!(
                        "parent cycle: entry {} is its own parent",
                        entry.entry_id
                    )),
                });
            }
            let Some(pi) = index_of(parent) else {
                return Err(SessionError::Corrupt {
                    path: path.to_path_buf(),
                    line,
                    source: serde_json::Error::custom(format!(
                        "missing parent {} for entry {}",
                        parent, entry.entry_id
                    )),
                });
            };
            if pi >= i {
                return Err(SessionError::Corrupt {
                    path: path.to_path_buf(),
                    line,
                    source: serde_json::Error::custom(format!(
                        "parent {} of entry {} must precede it (forward reference)",
                        parent, entry.entry_id
                    )),
                });
            }
        }
        Ok(())
    }

    pub async fn maintain(&self, id: &SessionId) -> Result<bool> {
        use std::io::{BufRead, Seek, SeekFrom, Write};
        let _lock_file = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;
        let path = self.session_path(id)?;
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        if file.metadata()?.len() < 8 * 1024 * 1024 {
            return Ok(false);
        }
        let mut reader = std::io::BufReader::new(file);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let header: Header = match serde_json::from_str(&line) {
            Ok(header) => header,
            Err(_) => return Ok(false),
        };
        if header.version != SUPPORTED_SESSION_VERSION || header.id != *id {
            return Ok(false);
        }
        let mut replacement = tempfile::NamedTempFile::new_in(&self.root_dir)?;
        serde_json::to_writer(&mut replacement, &header)?;
        writeln!(replacement)?;
        let header_end = replacement.stream_position()?;
        let mut previous = None;
        let mut next = 0u64;
        let mut boundary = false;
        loop {
            line.clear();

            let n = std::io::Read::take(&mut reader, 16 * 1024 * 1024 + 1).read_line(&mut line)?;
            if n == 0 {
                break;
            }
            if !line.ends_with('\n') || n > 16 * 1024 * 1024 {
                return Ok(false);
            }
            if line.trim().is_empty() {
                continue;
            }
            let mut entry: SessionEntry = match serde_json::from_str(&line) {
                Ok(entry) => entry,
                Err(_) => return Ok(false),
            };
            if entry.parent_id != previous || previous.is_some_and(|p| entry.entry_id <= p) {
                return Ok(false);
            }
            previous = Some(entry.entry_id);
            if entry.compaction_boundary {
                replacement.as_file().set_len(header_end)?;
                replacement.seek(SeekFrom::Start(header_end))?;
                next = 0;
                boundary = true;
                continue;
            }

            if !boundary {
                continue;
            }
            entry.entry_id = next;
            entry.parent_id = next.checked_sub(1);
            next += 1;
            serde_json::to_writer(&mut replacement, &entry)?;
            writeln!(replacement)?;
        }
        if !boundary {
            return Ok(false);
        }
        replacement.as_file().sync_all()?;
        self.archive_original(id, &path)?;
        replacement
            .persist(&path)
            .map_err(|e| SessionError::Io(e.error))?;
        Ok(true)
    }

    fn archive_original(&self, id: &SessionId, path: &Path) -> Result<()> {
        let archive_dir = self.root_dir.join("archive");
        ensure_private_dir(&archive_dir)?;
        let mut archive = tempfile::Builder::new()
            .prefix(&format!("{}-", id.as_str()))
            .suffix(".jsonl")
            .tempfile_in(&archive_dir)?;
        std::io::copy(&mut std::fs::File::open(path)?, &mut archive)?;
        archive.as_file().sync_all()?;
        let _ = archive.keep().map_err(|e| SessionError::Io(e.error))?;
        Ok(())
    }

    pub async fn rewind(&self, id: &SessionId, keep: usize) -> Result<Vec<SessionEntry>> {
        use std::io::Write;

        let (_meta, active) = self.load(id).await?;
        if keep >= active.len() {
            return Ok(Vec::new());
        }
        let dropped = active[keep..].to_vec();
        let _lock_file = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;
        let path = self.session_path(id)?;
        let content = std::fs::read_to_string(&path)?;
        let mut lines = content.lines().filter(|line| !line.trim().is_empty());
        let header_line = lines
            .next()
            .ok_or_else(|| SessionError::Io(std::io::Error::other("session file has no header")))?;
        let records: Vec<&str> = lines.collect();
        let superseded = records
            .iter()
            .rposition(|line| {
                serde_json::from_str::<SessionEntry>(line)
                    .map(|entry| entry.compaction_boundary)
                    .unwrap_or(false)
            })
            .map(|index| index + 1)
            .unwrap_or(0);
        let keep_records = superseded + keep;
        let mut replacement = tempfile::NamedTempFile::new_in(&self.root_dir)?;
        writeln!(replacement, "{header_line}")?;
        for record in &records[..keep_records] {
            writeln!(replacement, "{record}")?;
        }
        replacement.as_file().sync_all()?;
        self.archive_original(id, &path)?;
        replacement
            .persist(&path)
            .map_err(|e| SessionError::Io(e.error))?;
        Ok(dropped)
    }

    async fn read_session_text(path: &Path) -> Result<Option<(String, Option<Vec<u8>>)>> {
        let bytes = match tokio::fs::read(path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(SessionError::Io(e)),
        };
        match String::from_utf8(bytes) {
            Ok(text) => Ok(Some((text, None))),
            Err(error) => {
                let valid = error.utf8_error().valid_up_to();
                let rest = &error.as_bytes()[valid..];
                if rest.contains(&b'\n') {
                    return Err(SessionError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "session file contains invalid UTF-8 before the final record",
                    )));
                }
                let bytes = error.into_bytes();

                let prefix = std::str::from_utf8(&bytes[..valid])
                    .expect("prefix bounded by valid_up_to is valid UTF-8")
                    .to_owned();
                Ok(Some((prefix, Some(bytes))))
            }
        }
    }

    pub async fn load(&self, id: &SessionId) -> Result<(SessionMeta, Vec<SessionEntry>)> {
        let _lock_file = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;
        let path = self.session_path(id)?;
        let (content, raw) = match Self::read_session_text(&path).await? {
            Some(both) => both,
            None => return Err(SessionError::NotFound(id.clone())),
        };

        let all_lines: Vec<(usize, &str)> = content
            .lines()
            .enumerate()
            .map(|(idx, line)| (idx + 1, line))
            .filter(|(_, line)| !line.trim().is_empty())
            .collect();

        let (header_line_num, header_str) =
            all_lines.first().map(|&(n, s)| (n, s)).unwrap_or((1, ""));
        let header: Header = match serde_json::from_str(header_str) {
            Ok(h) => h,
            Err(e) => {
                Self::quarantine_corrupt_file(&path).await;
                return Err(SessionError::Corrupt {
                    path: path.clone(),
                    line: header_line_num,
                    source: e,
                });
            }
        };

        let mut damaged_tail = false;
        let entry_lines = &all_lines[1..];
        let parsed: Vec<std::result::Result<SessionEntry, serde_json::Error>> =
            if content.len() > (1 << 21) {
                use rayon::prelude::*;
                entry_lines
                    .par_iter()
                    .map(|(_, line_str)| serde_json::from_str::<SessionEntry>(line_str))
                    .collect()
            } else {
                entry_lines
                    .iter()
                    .map(|(_, line_str)| serde_json::from_str::<SessionEntry>(line_str))
                    .collect()
            };
        let mut entries = Vec::with_capacity(entry_lines.len());
        let mut entry_line_nums = Vec::with_capacity(entry_lines.len());

        for (idx, ((line_num, _), result)) in entry_lines.iter().zip(parsed).enumerate() {
            let is_final_line = idx == entry_lines.len() - 1;
            match result {
                Ok(entry) => {
                    entries.push(entry);
                    entry_line_nums.push(*line_num);
                }
                Err(e) => {
                    if is_final_line {
                        damaged_tail = true;
                        log::warn!(
                            "ignoring corrupt or torn final line in {}:{}: {}",
                            path.display(),
                            line_num,
                            e
                        );
                    } else {
                        return Err(SessionError::Corrupt {
                            path: path.clone(),
                            line: *line_num,
                            source: e,
                        });
                    }
                }
            }
        }

        Self::validate_loaded_graph(
            &path,
            id,
            header_line_num,
            &header,
            &entries,
            &entry_line_nums,
        )?;

        if damaged_tail || !content.ends_with('\n') {
            use std::io::Write;
            let mut backup = tempfile::Builder::new()
                .prefix("session-tail-backup-")
                .tempfile_in(&self.root_dir)?;
            backup.write_all(raw.as_deref().unwrap_or(content.as_bytes()))?;
            let (_, backup_path) = backup.keep().map_err(|e| SessionError::Io(e.error))?;
            let mut repaired = String::new();
            let keep = if damaged_tail {
                all_lines.len() - 1
            } else {
                all_lines.len()
            };
            for (_, line) in all_lines.iter().take(keep) {
                repaired.push_str(line);
                repaired.push('\n');
            }
            let mut replacement = tempfile::NamedTempFile::new_in(&self.root_dir)?;
            replacement.write_all(repaired.as_bytes())?;
            replacement.as_file().sync_all()?;
            replacement
                .persist(&path)
                .map_err(|e| SessionError::Io(e.error))?;
            log::warn!(
                "repaired session tail; original saved at {}",
                backup_path.display()
            );
        }

        let meta = SessionMeta {
            id: header.id,
            timestamp: header.timestamp,
            cwd: header.cwd.clone(),
            model: header.model,
            origin: header.origin,
        };
        if meta.origin.is_none() {
            self.remember(id, &meta.cwd).await;
        }

        let mut active = entries;
        if let Some(last) = active.iter().rposition(|e| e.compaction_boundary) {
            active.drain(..=last);
        }

        Ok((meta, active))
    }

    pub async fn list(&self) -> Vec<SessionSummary> {
        self.list_matching(None).await
    }

    pub async fn list_in(&self, cwd: &Path) -> Vec<SessionSummary> {
        self.list_matching(Some(cwd.to_path_buf())).await
    }

    async fn list_matching(&self, cwd: Option<PathBuf>) -> Vec<SessionSummary> {
        let mut rx = self.scan(cwd, false);
        let mut summaries = Vec::new();
        while let Some(event) = rx.recv().await {
            match event {
                SessionScanEvent::Summary(s) => summaries.push(s),
                SessionScanEvent::Done => break,
                SessionScanEvent::Running(_) => {}
            }
        }

        summaries.sort_by(|a, b| {
            b.last_message_at
                .cmp(&a.last_message_at)
                .then_with(|| b.id.as_str().cmp(a.id.as_str()))
        });
        summaries
    }

    pub(crate) fn scan(
        &self,
        cwd: Option<PathBuf>,
        probe_locks: bool,
    ) -> tokio::sync::mpsc::UnboundedReceiver<SessionScanEvent> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let root = self.root_dir.clone();
        tokio::spawn(scan_session_dir(root, cwd, probe_locks, tx));
        rx
    }

    pub async fn delete(&self, id: &SessionId) -> Result<()> {
        let _lock = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;

        if self.open_lock_held(id).await {
            return Err(SessionError::Locked {
                id: id.clone(),
                pid: self.open_holder_pid(id),
            });
        }
        let path = self.session_path(id)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(SessionError::Io(e)),
        }

        for ext in ["dismissed", "title", "autotitle"] {
            let sidecar = self.root_dir.join(format!("{}.{ext}", id.as_str()));
            let _ = tokio::fs::remove_file(&sidecar).await;
        }
        Ok(())
    }

    pub async fn prune_before(&self, cutoff_ms: u64) -> Result<Vec<SessionId>> {
        let mut removed = Vec::new();
        for summary in self.list().await {
            if summary.started_at < cutoff_ms {
                match self.delete(&summary.id).await {
                    Ok(()) => removed.push(summary.id),

                    Err(SessionError::Locked { .. }) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(removed)
    }
}

pub fn default_root() -> Option<PathBuf> {
    gray_core::paths::gray_home().map(|home| home.join("sessions"))
}

fn read_dismissal(root: &Path, id: &str) -> Option<u64> {
    std::fs::read_to_string(root.join(format!("{id}.dismissed")))
        .ok()?
        .trim()
        .parse()
        .ok()
}

pub fn write_dismissal(root: &Path, id: &SessionId, last_message_at: u64) -> std::io::Result<()> {
    if !valid_session_id(id.as_str()) {
        return Err(std::io::Error::other("invalid session id"));
    }
    std::fs::write(
        root.join(format!("{}.dismissed", id.as_str())),
        last_message_at.to_string(),
    )
}

const MAX_TITLE_CHARS: usize = 80;

pub(crate) fn clean_title(raw: &str) -> Option<String> {
    let one: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let t: String = one.chars().take(MAX_TITLE_CHARS).collect();
    (!t.is_empty()).then_some(t)
}

fn read_title(root: &Path, id: &str) -> Option<String> {
    clean_title(&std::fs::read_to_string(root.join(format!("{id}.title"))).ok()?)
}

pub(crate) fn opener_tag(first: &str) -> Option<&'static str> {
    let first = first.trim_start();
    let head: String = first.chars().take(160).collect();
    if first.starts_with(crate::composer::transcript::BG_NOTICE_MARKER) {
        Some("background")
    } else if head.contains("subagent of a parent Gray") {
        Some("subagent")
    } else if head.starts_with("You are") && head.contains(" worker") {
        Some("worker")
    } else {
        None
    }
}

pub fn claim_auto_title(root: &Path, id: &SessionId) -> bool {
    if !valid_session_id(id.as_str()) || read_title(root, id.as_str()).is_some() {
        return false;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(format!("{}.autotitle", id.as_str())))
        .is_ok()
}

pub fn set_auto_title(root: &Path, id: &SessionId, proposed: &str) -> std::io::Result<bool> {
    use std::io::Write as _;
    if !valid_session_id(id.as_str()) {
        return Err(std::io::Error::other("invalid session id"));
    }
    let Some(title) = clean_title(proposed) else {
        return Ok(false);
    };
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(format!("{}.title", id.as_str())))
    {
        Ok(mut file) => {
            file.write_all(title.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

pub fn write_title(root: &Path, id: &SessionId, title: &str) -> std::io::Result<Option<String>> {
    if !valid_session_id(id.as_str()) {
        return Err(std::io::Error::other("invalid session id"));
    }
    let path = root.join(format!("{}.title", id.as_str()));
    match clean_title(title) {
        Some(t) => {
            std::fs::write(&path, &t)?;
            Ok(Some(t))
        }
        None => {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
            Ok(None)
        }
    }
}

static OPEN_LOCKS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, std::sync::Weak<OpenLockInner>>>,
> = std::sync::LazyLock::new(Default::default);

#[derive(Debug)]
struct OpenLockInner {
    _file: std::fs::File,
}

#[derive(Debug)]
pub struct SessionOpenGuard {
    _inner: std::sync::Arc<OpenLockInner>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenHolder {
    pid: u32,
    started_ms: u64,
}

fn write_open_holder(file: &mut std::fs::File, path: &Path) {
    use std::io::{Seek, SeekFrom, Write};
    #[cfg(not(windows))]
    let _ = path;
    let holder = OpenHolder {
        pid: std::process::id(),
        started_ms: now_millis(),
    };
    let Ok(json) = serde_json::to_string(&holder) else {
        return;
    };
    let _ = file.seek(SeekFrom::Start(0));
    let _ = file.set_len(0);
    let _ = file.write_all(json.as_bytes());
    let _ = file.write_all(b"\n");
    let _ = file.sync_all();
    #[cfg(windows)]
    {
        let hint_path = path.with_extension("open-holder");
        let _ = std::fs::remove_file(&hint_path);
        if let Some(parent) = path.parent()
            && let Ok(mut hint) = tempfile::NamedTempFile::new_in(parent)
            && hint.write_all(json.as_bytes()).is_ok()
            && hint.as_file().sync_all().is_ok()
        {
            let _ = hint.persist(hint_path);
        }
    }
}

fn open_lock_held_at(open_path: &Path) -> bool {
    if let Ok(held) = OPEN_LOCKS.lock()
        && held
            .get(open_path)
            .and_then(std::sync::Weak::upgrade)
            .is_some()
    {
        return true;
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(open_path)
    else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => false,
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(std::fs::TryLockError::Error(_)) => false,
    }
}

/// On Windows `/restart` spawns the new process while the old one is
/// still exiting, so the session open-lock can briefly remain held by
/// the dying parent. Give the holder a short grace period to release
/// the lock before reporting the session as locked. A restarting parent
/// that sets RESTART_PARENT_ENV gets a longer budget, and a lock held
/// by any other pid then fails immediately; without the hint (e.g. the
/// parent is an older build) a few seconds still covers the exit.
async fn wait_out_restart_parent(file: &mut std::fs::File, open_path: &Path) -> bool {
    let wait_pid = RESTART_PARENT_PID.get().copied().flatten();
    let budget = if wait_pid.is_some() {
        RESTART_PARENT_WAIT
    } else {
        std::time::Duration::from_secs(3)
    };
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        match file.try_lock() {
            Ok(()) => return true,
            Err(std::fs::TryLockError::WouldBlock) => {
                if let (Some(wp), Some(holder)) = (wait_pid, read_open_holder(open_path))
                    && holder != wp
                {
                    return false;
                }
            }
            Err(std::fs::TryLockError::Error(e)) => {
                log::warn!(
                    "session open-lock unsupported on {} ({e}); proceeding unlocked",
                    open_path.display()
                );
                return true;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Releases the OS-level lock on every session open-file this process
/// holds, without dropping the guards. Windows `/restart` needs this:
/// the spawned child has to acquire the same lock while the parent is
/// still alive supervising it. The parent stays in `child.wait()` so
/// the launching shell cannot reclaim the console and steal input.
#[cfg(not(unix))]
pub(crate) fn release_open_locks() {
    let Ok(held) = OPEN_LOCKS.lock() else {
        return;
    };
    for inner in held.values().filter_map(std::sync::Weak::upgrade) {
        if let Err(e) = inner._file.unlock() {
            log::warn!("failed to release session open-lock before restart: {e}");
        }
    }
}

fn read_open_holder(path: &Path) -> Option<u32> {
    #[cfg(windows)]
    let path = path.with_extension("open-holder");
    let content = std::fs::read_to_string(path).ok()?;
    let content = content.trim();
    serde_json::from_str::<OpenHolder>(content)
        .ok()
        .map(|h| h.pid)
        .or_else(|| content.parse::<u32>().ok())
}

#[path = "session_store_tests.rs"]
#[cfg(test)]
mod tests;
