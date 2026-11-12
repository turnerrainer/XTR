import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.util.*;
import java.util.regex.Pattern;
import javax.xml.XMLConstants;
import javax.xml.namespace.QName;
import javax.xml.transform.stream.StreamSource;
import javax.xml.validation.*;
import org.apache.xerces.jaxp.SAXParserFactoryImpl;
import org.apache.xerces.jaxp.validation.XMLSchemaFactory;
import org.w3c.dom.ls.LSInput;
import org.w3c.dom.ls.LSResourceResolver;
import org.xml.sax.*;
import org.xml.sax.helpers.DefaultHandler;

/**
 * Xerces oracle for the XSD validator tests.
 *
 * Usage: java Oracle SCHEMA_DIR MESSAGE_DIR | java Oracle --info
 *
 * It prints one JSON document. It never prints a message value or a Xerces
 * message text. It reads only files below SCHEMA_DIR and MESSAGE_DIR.
 */
public final class Oracle {
    static final String XERCES = "xerces-2.12.2";
    static final Pattern SCHEME = Pattern.compile("^[A-Za-z][A-Za-z0-9+.\\-]*:.*", Pattern.DOTALL);
    static final Pattern CODE = Pattern.compile("^(cvc|src|s4s|sch|schema_reference|[A-Za-z]+)[-A-Za-z0-9_.]*");

    /** Refusal of a schema reference or a DOCTYPE. */
    static final class Refused extends RuntimeException {
        final String cls;
        Refused(String cls) { super(cls); this.cls = cls; }
    }

    static Path root;

    /** Reads a file below the schema directory. Refuses everything else. */
    static byte[] readLocal(String ref) throws IOException {
        if (ref == null || ref.isEmpty()) throw new Refused("resolver_refused");
        if (SCHEME.matcher(ref).matches()) throw new Refused("resolver_refused");
        if (ref.indexOf('%') >= 0 || ref.indexOf('\\') >= 0 || ref.indexOf('\0') >= 0) throw new Refused("resolver_refused");
        if (ref.startsWith("/") || ref.startsWith("?") || ref.contains("#")) throw new Refused("resolver_refused");
        for (String seg : ref.split("/")) {
            if (seg.equals("..")) throw new Refused("resolver_refused");
        }
        Path p = root.resolve(ref).normalize();
        if (!p.startsWith(root)) throw new Refused("resolver_refused");
        Path real = p.toRealPath();
        if (!real.startsWith(root)) throw new Refused("resolver_refused");
        if (!Files.isRegularFile(real)) throw new Refused("resolver_refused");
        byte[] b = Files.readAllBytes(real);
        rejectDoctype(b);
        return b;
    }

    static void rejectDoctype(byte[] b) {
        // Remove NUL bytes so that UTF-16 text is also checked.
        StringBuilder sb = new StringBuilder(b.length);
        for (byte x : b) if (x != 0) sb.append((char) (x & 0xff));
        if (sb.indexOf("<!DOCTYPE") >= 0) throw new Refused("doctype_refused");
    }

    static final class Resolver implements LSResourceResolver {
        public LSInput resolveResource(String type, String ns, String publicId, String systemId, String baseURI) {
            try {
                final byte[] data = readLocal(systemId);
                return new LSInput() {
                    public Reader getCharacterStream() { return null; }
                    public void setCharacterStream(Reader r) {}
                    public InputStream getByteStream() { return new ByteArrayInputStream(data); }
                    public void setByteStream(InputStream s) {}
                    public String getStringData() { return null; }
                    public void setStringData(String s) {}
                    public String getSystemId() { return "file:///schema/" + systemId; }
                    public void setSystemId(String s) {}
                    public String getPublicId() { return null; }
                    public void setPublicId(String s) {}
                    public String getBaseURI() { return null; }
                    public void setBaseURI(String s) {}
                    public String getEncoding() { return null; }
                    public void setEncoding(String s) {}
                    public boolean getCertifiedText() { return false; }
                    public void setCertifiedText(boolean b) {}
                };
            } catch (IOException e) {
                throw new Refused("resolver_refused");
            }
        }
    }

    static String json(String s) {
        StringBuilder b = new StringBuilder("\"");
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            if (c == '"' || c == '\\') b.append('\\').append(c);
            else if (c < 0x20 || c > 0x7e) b.append(String.format("\\u%04x", (int) c));
            else b.append(c);
        }
        return b.append('"').toString();
    }

    /** Attribute name (never a value) from the Xerces message of an attribute error. */
    static String attributeName(String code, String message) {
        if (message == null) return null;
        java.util.regex.Matcher m;
        if (code.equals("cvc-complex-type.3.2.2") || code.equals("cvc-complex-type.4")) {
            m = Pattern.compile("^[^:]+: Attribute '([^']*)'").matcher(message);
        } else if (code.equals("cvc-attribute.3") || code.equals("cvc-complex-type.3.1") || code.equals("cvc-attribute.4")) {
            m = Pattern.compile(" of attribute '([^']*)'").matcher(message);
        } else if (code.equals("cvc-type.3.1.1")) {
            m = Pattern.compile("the attribute, '([^']*)'").matcher(message);
        } else {
            return null;
        }
        return m.find() ? m.group(1) : null;
    }

    static String codeOf(String message) {
        if (message == null) return "unknown";
        java.util.regex.Matcher m = Pattern.compile("^([A-Za-z0-9_.\\-]+?):").matcher(message);
        return m.find() ? m.group(1) : "unknown";
    }

    static SchemaFactory factory() throws Exception {
        SchemaFactory f = new XMLSchemaFactory();
        f.setFeature(XMLConstants.FEATURE_SECURE_PROCESSING, true);
        // Xerces 2.12.2 does not know the JDK access properties. The resolver
        // below is the only way that a schema reference is read.
        f.setResourceResolver(new Resolver());
        f.setErrorHandler(new ErrorHandler() {
            public void warning(SAXParseException e) {}
            public void error(SAXParseException e) throws SAXException { throw e; }
            public void fatalError(SAXParseException e) throws SAXException { throw e; }
        });
        return f;
    }

    /** Top-level element names (as "{ns}local") of one schema file. */
    static Set<String> topElements(byte[] xsd) throws Exception {
        SAXParserFactoryImpl pf = new SAXParserFactoryImpl();
        pf.setNamespaceAware(true);
        final Set<String> out = new TreeSet<>();
        final String[] tns = {""};
        final int[] depth = {0};
        pf.newSAXParser().parse(new ByteArrayInputStream(xsd), new DefaultHandler() {
            public void startElement(String uri, String local, String q, Attributes a) {
                if (depth[0] == 0 && local.equals("schema")) {
                    String t = a.getValue("targetNamespace");
                    tns[0] = t == null ? "" : t;
                } else if (depth[0] == 1 && local.equals("element") && uri.equals(XMLConstants.W3C_XML_SCHEMA_NS_URI)) {
                    String n = a.getValue("name");
                    if (n != null) out.add("{" + tns[0] + "}" + n);
                }
                depth[0]++;
            }
            public void endElement(String uri, String local, String q) { depth[0]--; }
        });
        return out;
    }

    /** For each element in document order: how many siblings with the same name its parent has. */
    static int[] siblingTotals(byte[] data) throws Exception {
        SAXParserFactoryImpl pf = new SAXParserFactoryImpl();
        pf.setNamespaceAware(true);
        final ArrayList<Integer> out = new ArrayList<>();
        final ArrayList<HashMap<String, List<Integer>>> frames = new ArrayList<>();
        frames.add(new HashMap<>());
        pf.newSAXParser().parse(new ByteArrayInputStream(data), new DefaultHandler() {
            public void startElement(String uri, String local, String q, Attributes a) {
                int id = out.size();
                out.add(1);
                frames.get(frames.size() - 1).computeIfAbsent("{" + uri + "}" + local, k -> new ArrayList<>()).add(id);
                frames.add(new HashMap<>());
            }
            public void endElement(String uri, String local, String q) {
                HashMap<String, List<Integer>> f = frames.remove(frames.size() - 1);
                for (List<Integer> ids : f.values()) for (int id : ids) out.set(id, ids.size());
            }
        });
        int[] r = new int[out.size()];
        for (int i = 0; i < r.length; i++) r[i] = out.get(i);
        return r;
    }

    /** Tracks the element path before the validator sees an event. */
    static final class Tracker extends DefaultHandler {
        final ValidatorHandler vh;
        final ArrayList<String> names = new ArrayList<>();
        final ArrayList<HashMap<String, Integer>> counts = new ArrayList<>();
        final ArrayList<Integer> index = new ArrayList<>();
        final ArrayList<Integer> ordinals = new ArrayList<>();
        final int[] totals;
        int next = 0;
        String attrSuffix = "";
        Locator loc;
        int cn = 0;

        Tracker(ValidatorHandler vh, int[] totals) { this.vh = vh; this.totals = totals; counts.add(new HashMap<>()); }
        public void setDocumentLocator(Locator l) { loc = l; vh.setDocumentLocator(l); }
        public void startDocument() throws SAXException { vh.startDocument(); }
        public void endDocument() throws SAXException { vh.endDocument(); }
        public void startPrefixMapping(String p, String u) throws SAXException { vh.startPrefixMapping(p, u); }
        public void endPrefixMapping(String p) throws SAXException { vh.endPrefixMapping(p); }
        public void startElement(String uri, String local, String q, Attributes a) throws SAXException {
            HashMap<String, Integer> c = counts.get(counts.size() - 1);
            String key = "{" + uri + "}" + local;
            int n = c.merge(key, 1, Integer::sum);
            names.add(local);
            index.add(n);
            ordinals.add(next++);
            counts.add(new HashMap<>());
            // The oracle ignores xsi:schemaLocation and xsi:noNamespaceSchemaLocation.
            org.xml.sax.helpers.AttributesImpl clean = new org.xml.sax.helpers.AttributesImpl();
            for (int i = 0; i < a.getLength(); i++) {
                String an = a.getLocalName(i);
                boolean hint = a.getURI(i).equals(XMLConstants.W3C_XML_SCHEMA_INSTANCE_NS_URI)
                    && (an.equals("schemaLocation") || an.equals("noNamespaceSchemaLocation"));
                if (!hint) clean.addAttribute(a.getURI(i), an, a.getQName(i), a.getType(i), a.getValue(i));
            }
            vh.startElement(uri, local, q, clean);
        }
        public void endElement(String uri, String local, String q) throws SAXException {
            vh.endElement(uri, local, q);
            names.remove(names.size() - 1);
            index.remove(index.size() - 1);
            ordinals.remove(ordinals.size() - 1);
            counts.remove(counts.size() - 1);
        }
        public void characters(char[] ch, int s, int l) throws SAXException {
            for (int i = s; i < s + l; i++) {
                int c = ch[i];
                if (Character.isHighSurrogate((char) c) && i + 1 < s + l) { c = Character.toCodePoint((char) c, ch[++i]); }
                if (Character.getType(c) == Character.UNASSIGNED) cn++;
            }
            vh.characters(ch, s, l);
        }
        public void ignorableWhitespace(char[] ch, int s, int l) throws SAXException { vh.ignorableWhitespace(ch, s, l); }
        public void processingInstruction(String t, String d) throws SAXException { vh.processingInstruction(t, d); }

        /** Path of the current element. Index is added when the name repeats under one parent. */
        String path(int repeatsAbove) {
            StringBuilder b = new StringBuilder();
            for (int i = 0; i < names.size(); i++) {
                b.append('/').append(names.get(i));
                if (totals[ordinals.get(i)] > 1) b.append('[').append(index.get(i)).append(']');
            }
            return b.length() == 0 ? "/" : b.toString();
        }
    }

    static final class Verdict {
        String verdict = "valid";
        String schema = null;
        int cn = 0;
        final List<String[]> errors = new ArrayList<>();
    }

    static String errorsJson(List<String[]> errors) {
        StringBuilder b = new StringBuilder();
        for (String[] e : errors) {
            if (b.length() > 0) b.append(',');
            b.append("{\"path\":").append(json(e[0])).append(",\"code\":").append(json(e[1])).append(",\"line\":").append(e[2]).append('}');
        }
        return b.toString();
    }

    static Verdict check(Path msg, Map<String, Schema> compiled, Map<String, List<String>> byRoot) throws Exception {
        Verdict v = new Verdict();
        byte[] data = Files.readAllBytes(msg);
        try {
            rejectDoctype(data);
        } catch (Refused r) {
            v.verdict = "refused";
            return v;
        }
        // Find the root name without validation.
        SAXParserFactoryImpl pf = new SAXParserFactoryImpl();
        pf.setNamespaceAware(true);
        final String[] rootName = {null};
        try {
            pf.newSAXParser().parse(new ByteArrayInputStream(data), new DefaultHandler() {
                int d = 0;
                public void startElement(String uri, String local, String q, Attributes a) {
                    if (d++ == 0) rootName[0] = "{" + uri + "}" + local;
                }
                public void endElement(String uri, String l, String q) { d--; }
            });
        } catch (SAXException | IOException e) {
            v.verdict = "not_wellformed";
            return v;
        }
        List<String> files = byRoot.get(rootName[0]);
        if (files == null || files.isEmpty()) {
            v.verdict = "no_schema";
            return v;
        }
        v.schema = files.get(0);
        Schema schema = compiled.get(v.schema);
        final ValidatorHandler vh = schema.newValidatorHandler();
        vh.setFeature(XMLConstants.FEATURE_SECURE_PROCESSING, true);
        vh.setResourceResolver(new Resolver());
        final Tracker t = new Tracker(vh, siblingTotals(data));
        vh.setErrorHandler(new ErrorHandler() {
            void add(SAXParseException e) {
                String path = t.path(0);
                String code = codeOf(e.getMessage());
                String attr = attributeName(code, e.getMessage());
                // The facet error comes first. The wrapper error names the attribute.
                if (code.equals("cvc-attribute.3") && attr != null && !v.errors.isEmpty()) {
                    String[] last = v.errors.get(v.errors.size() - 1);
                    if (last[0].equals(path)) last[0] = path + "/@" + attr;
                }
                if (attr != null && !code.equals("cvc-attribute.3")) path = path + "/@" + attr;
                // Xerces reports xsi attributes on the element. The Rust validator names the attribute.
                if (code.startsWith("cvc-elt.3.")) path = path + "/@nil";
                if (code.startsWith("cvc-elt.4.")) path = path + "/@type";
                v.errors.add(new String[] {path, code, Integer.toString(e.getLineNumber())});
            }
            public void warning(SAXParseException e) {}
            public void error(SAXParseException e) { add(e); }
            public void fatalError(SAXParseException e) throws SAXException { throw e; }
        });
        SAXParserFactoryImpl mf = new SAXParserFactoryImpl();
        mf.setNamespaceAware(true);
        mf.setFeature("http://apache.org/xml/features/disallow-doctype-decl", true);
        try {
            mf.newSAXParser().parse(new ByteArrayInputStream(data), t);
        } catch (SAXParseException e) {
            v.verdict = "not_wellformed";
            v.errors.clear();
            v.cn = t.cn;
            return v;
        }
        v.cn = t.cn;
        if (!v.errors.isEmpty()) v.verdict = "invalid";
        return v;
    }

    public static void main(String[] args) throws Exception {
        if (args.length == 1 && args[0].equals("--info")) {
            int[] probes = {0x2C2F, 0x31350, 0x2EBF0, 0x10D50};
            StringBuilder b = new StringBuilder("{\"oracle\":" + json(XERCES) + ",\"java\":" + json(System.getProperty("java.version")) + ",\"unicode_probe\":{");
            for (int i = 0; i < probes.length; i++) {
                if (i > 0) b.append(',');
                b.append(json(String.format("%X", probes[i]))).append(':').append(json(Character.getType(probes[i]) == Character.UNASSIGNED ? "Cn" : Character.isLetter(probes[i]) ? "letter" : "other"));
            }
            System.out.println(b.append("}}"));
            return;
        }
        if (args.length != 2) {
            System.err.println("usage: Oracle SCHEMA_DIR MESSAGE_DIR");
            System.exit(2);
        }
        root = Paths.get(args[0]).toRealPath();
        Path msgDir = Paths.get(args[1]).toRealPath();
        SchemaFactory f = factory();
        Map<String, Schema> compiled = new TreeMap<>();
        Map<String, List<String>> byRoot = new TreeMap<>();
        StringBuilder schemas = new StringBuilder();
        List<Path> xsds = new ArrayList<>();
        try (DirectoryStream<Path> ds = Files.newDirectoryStream(root, "*.xsd")) { for (Path p : ds) xsds.add(p); }
        Collections.sort(xsds);
        for (Path p : xsds) {
            String name = p.getFileName().toString();
            String status = "ok", cls = "";
            try {
                byte[] b = readLocal(name);
                Schema s = f.newSchema(new StreamSource(new ByteArrayInputStream(b), "file:///schema/" + name));
                compiled.put(name, s);
                for (String e : topElements(b)) byRoot.computeIfAbsent(e, k -> new ArrayList<>()).add(name);
            } catch (Refused r) {
                status = "error"; cls = r.cls;
            } catch (SAXParseException e) {
                status = "error"; cls = codeOf(e.getMessage());
                if (e.getException() instanceof Refused) cls = ((Refused) e.getException()).cls;
            } catch (Exception e) {
                Throwable c = e;
                while (c != null && !(c instanceof Refused)) c = c.getCause();
                status = "error"; cls = c == null ? "schema_error" : ((Refused) c).cls;
            }
            if (schemas.length() > 0) schemas.append(',');
            schemas.append("{\"file\":").append(json(name)).append(",\"status\":").append(json(status)).append(",\"class\":").append(json(cls)).append('}');
        }
        List<Path> msgs = new ArrayList<>();
        try (DirectoryStream<Path> ds = Files.newDirectoryStream(msgDir, "*.xml")) { for (Path p : ds) msgs.add(p); }
        Collections.sort(msgs);
        StringBuilder res = new StringBuilder();
        for (Path m : msgs) {
            Verdict v = check(m, compiled, byRoot);
            if (res.length() > 0) res.append(',');
            res.append("{\"file\":").append(json(m.getFileName().toString()))
               .append(",\"verdict\":").append(json(v.verdict))
               .append(",\"schema\":").append(v.schema == null ? "null" : json(v.schema))
               .append(",\"cn_chars\":").append(v.cn)
               .append(",\"errors\":[").append(errorsJson(v.errors)).append("]}");
        }
        System.out.println("{\"oracle\":" + json(XERCES) + ",\"schemas\":[" + schemas + "],\"results\":[" + res + "]}");
    }
}
