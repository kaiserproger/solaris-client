// Analysis-only tool. Never invoked by the native client. Its output is Mojang-derived
// local data and must remain outside source control/releases.
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.client.model.geom.*;
import net.minecraft.client.model.geom.builders.*;

public final class ExportModels {
    static final Map<String,Integer> invalidFaces = new TreeMap<>();
    static boolean normalsOnly;
    static final Field CUBES, CHILDREN;
    static {
        try {
            CUBES = ModelPart.class.getDeclaredField("cubes");
            CHILDREN = ModelPart.class.getDeclaredField("children");
            CUBES.setAccessible(true); CHILDREN.setAccessible(true);
        } catch (ReflectiveOperationException e) { throw new ExceptionInInitializerError(e); }
    }
    static Map<String,Object> dimension(net.minecraft.world.entity.EntityDimensions d) throws Exception {
        Map<String,Object> out=new LinkedHashMap<>();
        out.put("width",d.width());out.put("height",d.height());out.put("eye_height",d.eyeHeight());
        out.put("fixed",d.fixed());
        Field field=net.minecraft.world.entity.EntityAttachments.class.getDeclaredField("attachments");field.setAccessible(true);
        Map<String,Object> points=new TreeMap<>();
        for(var entry:((Map<?,?>)field.get(d.attachments())).entrySet()) {
            List<double[]> vectors=new ArrayList<>();
            for(Object object:(List<?>)entry.getValue()) {
                var point=(net.minecraft.world.phys.Vec3)object;vectors.add(new double[]{point.x,point.y,point.z});
            }
            points.put(entry.getKey().toString(),vectors);
        }
        out.put("attachments",points);
        return out;
    }
    static Object dimensionValue(Object value) throws Exception {
        if(value instanceof net.minecraft.world.entity.EntityDimensions d) return dimension(d);
        if(value instanceof Map<?,?> map) {
            Map<String,Object> out=new TreeMap<>();
            for(var entry:map.entrySet())
                if(entry.getKey() instanceof net.minecraft.world.entity.Pose pose&&entry.getValue() instanceof net.minecraft.world.entity.EntityDimensions d)
                    out.put(pose.name(),dimension(d));
            return out.isEmpty()?null:out;
        }
        return null;
    }
    static int enumId(Enum<?> value) throws Exception {
        for(String name:List.of("getId","id"))
            try { Method m=value.getClass().getMethod(name);m.setAccessible(true); if(m.getReturnType()==int.class) return (int)m.invoke(value); } catch(NoSuchMethodException ignored) {}
        return value.ordinal();
    }
    static Object materialValue(Object value,int depth) throws Exception {
        if (value==null || depth>6) return null;
        if (value instanceof net.minecraft.resources.Identifier) return value.toString();
        if (value instanceof Map<?,?> map) {
            Map<String,Object> out=new TreeMap<>();
            for(var e:map.entrySet()) { Object v=materialValue(e.getValue(),depth+1); if(v!=null) out.put(e.getKey().toString(),v); }
            Map<String,String> ids=new TreeMap<>();
            for(var e:map.entrySet())
                if(e.getKey() instanceof Enum<?> key) ids.put(Integer.toString(enumId(key)),key.toString());
            if(!ids.isEmpty()) out.put("__ids",ids);
            return out.isEmpty()?null:out;
        }
        if (value.getClass().isArray()) {
            List<Object> out=new ArrayList<>();
            for(int i=0;i<Array.getLength(value);i++) out.add(materialValue(Array.get(value,i),depth+1));
            return out;
        }
        if (value.getClass().isRecord()) {
            Map<String,Object> out=new TreeMap<>();
            for(var c:value.getClass().getRecordComponents()) {
                Method method=c.getAccessor();method.setAccessible(true);
                Object v=materialValue(method.invoke(value),depth+1);if(v!=null) out.put(c.getName(),v);
            }
            return out.isEmpty()?null:out;
        }
        return null;
    }
    static Map<String,Object> rendererDefaults(Map<String,Object> roots) throws Exception {
        // Instrumented analysis context: only model topology/static material access is allowed.
        // No submit, client world, texture allocation, native window, or GPU method is invoked.
        Field unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");unsafeField.setAccessible(true);
        sun.misc.Unsafe unsafe=(sun.misc.Unsafe)unsafeField.get(null);
        Class<?> contextClass=net.minecraft.client.renderer.entity.EntityRendererProvider.Context.class;
        var context=(net.minecraft.client.renderer.entity.EntityRendererProvider.Context)unsafe.allocateInstance(contextClass);
        Field modelSet=contextClass.getDeclaredField("modelSet");modelSet.setAccessible(true);
        modelSet.set(context,net.minecraft.client.model.geom.EntityModelSet.vanilla());
        Field providers=net.minecraft.client.renderer.entity.EntityRenderers.class.getDeclaredField("PROVIDERS");providers.setAccessible(true);
        Map<?,?> entries=(Map<?,?>)providers.get(null);
        Map<String,Object> results=new TreeMap<>();
        for(var entry:entries.entrySet()) {
            var type=(net.minecraft.world.entity.EntityType<?>)entry.getKey();
            String name=net.minecraft.core.registries.BuiltInRegistries.ENTITY_TYPE.getKey(type).toString();
            Map<String,Object> result=new LinkedHashMap<>();
            try {
                var renderer=((net.minecraft.client.renderer.entity.EntityRendererProvider<?>)entry.getValue()).create(context);
                result.put("renderer_class",renderer.getClass().getName());
                if(renderer instanceof net.minecraft.client.renderer.entity.LivingEntityRenderer living) {
                    roots.put(name+"#renderer_main",part("root",living.getModel().root()));
                    result.put("rig",name+"#renderer_main");
                    if (!normalsOnly) {
                        var state=(net.minecraft.client.renderer.entity.state.LivingEntityRenderState)living.createRenderState();
                        try {result.put("texture",living.getTextureLocation(state).toString());}catch(Throwable e){result.put("texture_error",e.toString());}
                    }
                }
                Map<String,Object> materials=new TreeMap<>();
                for(Class<?> c=renderer.getClass();c!=null;c=c.getSuperclass())
                    for(Field f:c.getDeclaredFields())
                        {
                            f.setAccessible(true);
                            Object object=f.get(Modifier.isStatic(f.getModifiers())?null:renderer);
                            if(object instanceof net.minecraft.client.model.Model model) {
                                String key=name+"#renderer/"+f.getName();
                                roots.put(key,part("root",model.root()));
                                if(f.getName().equals("model")&&!result.containsKey("rig")) result.put("rig",key);
                            }
                            if (!normalsOnly) {
                                Object v=materialValue(object,0);if(v!=null) materials.put(f.getName(),v);
                                if(f.getName().equals("texture")&&object instanceof net.minecraft.resources.Identifier) result.put("texture",object.toString());
                            }
                        }
                result.put("materials",materials);
            } catch(Throwable e) {result.put("error",e.toString());}
            results.put(name,result);
        }
        return results;
    }
    static Map<String,Object> part(String name, ModelPart p) throws Exception {
        Map<String,Object> out = new LinkedHashMap<>();
        out.put("name", name);
        if (!normalsOnly) {
            out.put("pivot", new float[]{p.x/16, p.y/16, p.z/16});
            out.put("rotation", new float[]{p.xRot,p.yRot,p.zRot});
            out.put("scale", new float[]{p.xScale,p.yScale,p.zScale});
        }
        List<float[][]> quads = normalsOnly ? null : new ArrayList<>();
        List<float[]> normals = new ArrayList<>();
        for (ModelPart.Cube c : (List<ModelPart.Cube>) CUBES.get(p)) {
            for (ModelPart.Polygon polygon : c.polygons) {
                float[][] q = normalsOnly ? null : new float[4][5];
                boolean finite = true;
                for (int i=0;i<4;i++) {
                    ModelPart.Vertex v = polygon.vertices()[i];
                    finite &= Float.isFinite(v.worldX()) && Float.isFinite(v.worldY())
                        && Float.isFinite(v.worldZ()) && Float.isFinite(v.u()) && Float.isFinite(v.v());
                    if (!normalsOnly) q[i] = new float[]{v.worldX(),v.worldY(),v.worldZ(),v.u(),v.v()};
                }
                if (finite) {
                    if (!normalsOnly) quads.add(q);
                    var normal = polygon.normal();
                    normals.add(new float[]{normal.x(),normal.y(),normal.z()});
                } else invalidFaces.merge(name,1,Integer::sum);
            }
        }
        if (!normalsOnly) out.put("quads",quads);
        out.put("normals",normals);
        List<Object> children = new ArrayList<>();
        for (var entry : ((Map<String,ModelPart>)CHILDREN.get(p)).entrySet())
            children.add(part(entry.getKey(),entry.getValue()));
        out.put("children", children);
        return out;
    }
    static int mergeNormals(JsonObject cached, JsonObject captured, String path) {
        if (!cached.get("name").equals(captured.get("name")))
            throw new IllegalStateException("normal part name mismatch: "+path);
        JsonArray normals=captured.getAsJsonArray("normals");
        if (cached.getAsJsonArray("quads").size()!=normals.size())
            throw new IllegalStateException("normal face count mismatch: "+path);
        cached.add("normals",normals);
        JsonArray oldChildren=cached.getAsJsonArray("children");
        Map<String,JsonObject> children=new HashMap<>();
        for (JsonElement child:captured.getAsJsonArray("children")) {
            JsonObject node=child.getAsJsonObject();
            if(children.put(node.get("name").getAsString(),node)!=null)
                throw new IllegalStateException("duplicate normal child: "+path);
        }
        if(oldChildren.size()!=children.size())
            throw new IllegalStateException("normal child count mismatch: "+path);
        int faces=normals.size();
        for(JsonElement child:oldChildren) {
            JsonObject node=child.getAsJsonObject();
            String name=node.get("name").getAsString();
            JsonObject actual=children.remove(name);
            if(actual==null)throw new IllegalStateException("missing normal child: "+path+"/"+name);
            faces+=mergeNormals(node,actual,path+"/"+name);
        }
        return faces;
    }
    static String sha256(Path path) throws Exception {
        return HexFormat.of().formatHex(java.security.MessageDigest.getInstance("SHA-256").digest(Files.readAllBytes(path)));
    }
    static void augmentNormals(String[] args) throws Exception {
        Path cache=Path.of(args[1]);
        JsonObject document=JsonParser.parseString(Files.readString(cache)).getAsJsonObject();
        if(!"26.1.2".equals(document.get("game_version").getAsString())
            || !args[2].equals(document.get("client_sha256").getAsString()))
            throw new IllegalStateException("normal augmentation cache Source provenance mismatch");
        String previousDigest=sha256(cache);
        normalsOnly=true;
        Map<String,Object> roots=new TreeMap<>();
        for(var entry:LayerDefinitions.createRoots().entrySet())
            roots.put(entry.getKey().toString(),part("root",entry.getValue().bakeRoot()));
        rendererDefaults(roots);
        JsonObject actual=new Gson().toJsonTree(roots).getAsJsonObject();
        JsonObject cached=document.getAsJsonObject("roots");
        if(!cached.keySet().equals(actual.keySet()))
            throw new IllegalStateException("normal augmentation root identity mismatch");
        int faces=0;
        for(String key:cached.keySet())
            faces+=mergeNormals(cached.getAsJsonObject(key),actual.getAsJsonObject(key),key);
        document.addProperty("schema",2);
        document.addProperty("source_api",document.get("source_api").getAsString()+"; Polygon.normal per retained finite face");
        Files.writeString(cache,new GsonBuilder().serializeNulls().create().toJson(document));
        Path provenancePath=cache.resolveSibling("provenance.json");
        JsonObject provenance=JsonParser.parseString(Files.readString(provenancePath)).getAsJsonObject();
        provenance.addProperty("schema",2);
        provenance.addProperty("models_sha256",sha256(cache));
        provenance.addProperty("normals_parent_models_sha256",previousDigest);
        provenance.addProperty("normals_exporter_sha256",sha256(Path.of(args[3])));
        provenance.addProperty("normals",faces);
        Files.writeString(provenancePath,new GsonBuilder().setPrettyPrinting().create().toJson(provenance)+"\n");
        System.out.println("normal augmentation roots="+roots.size()+" retained_faces="+faces+" models_sha256="+sha256(cache));
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        if(args[0].equals("--augment-normals")) { augmentNormals(args); return; }
        Map<String,Object> roots = new TreeMap<>();
        for (var entry : LayerDefinitions.createRoots().entrySet())
            roots.put(entry.getKey().toString(),part("root",entry.getValue().bakeRoot()));
        Map<String,Object> entities = new TreeMap<>();
        for (Field f : net.minecraft.world.entity.EntityType.class.getFields()) {
            if (f.getType()!=net.minecraft.world.entity.EntityType.class) continue;
            var type = (net.minecraft.world.entity.EntityType<?>)f.get(null);
            Type argument = ((ParameterizedType)f.getGenericType()).getActualTypeArguments()[0];
            if (!(argument instanceof Class<?>)) continue;
            Class<?> entityClass = (Class<?>)argument;
            Map<String,Object> accessors = new TreeMap<>();
            Map<String,Object> dimensions=new TreeMap<>();
            Map<String,Object> enums=new TreeMap<>();
            Map<String,Object> enumNumbers=new TreeMap<>();
            List<String> hierarchy=new ArrayList<>();
            Map<String,String> methods=new TreeMap<>();
            for (Class<?> c=entityClass;c!=null;c=c.getSuperclass()) {
                hierarchy.add(c.getName());
                for(Method method:c.getDeclaredMethods())
                    if(List.of("getDefaultDimensions","getAgeScale","getPickRadius","isPickable","canBeCollidedWith","canCollideWith","isPushable","makeBoundingBox","sanitizeScale","modifyPassengerFluidInteractionBox","getPassengerAttachmentPoint","getPassengerRidingPosition","getControllingPassenger","mobInteract","interact","interactAt").contains(method.getName()))
                        methods.putIfAbsent(method.getName(),c.getName());
                for (Field a:c.getDeclaredFields()) {
                    if(Modifier.isStatic(a.getModifiers())) {
                        a.setAccessible(true);Object d=dimensionValue(a.get(null));if(d!=null) dimensions.putIfAbsent(a.getName(),d);
                    }
                    if (a.getType()!=net.minecraft.network.syncher.EntityDataAccessor.class || !Modifier.isStatic(a.getModifiers())) continue;
                    a.setAccessible(true);
                    var accessor=(net.minecraft.network.syncher.EntityDataAccessor<?>)a.get(null);
                    accessors.put(a.getName(),new int[]{accessor.id(),net.minecraft.network.syncher.EntityDataSerializers.getSerializedId(accessor.serializer())});
                }
            }
            for(Class<?> c=entityClass;c!=null;c=c.getSuperclass())
                for(Class<?> nested:c.getDeclaredClasses())
                    if(nested.isEnum()) {
                        Map<String,String> ids=new TreeMap<>();
                        for(Object value:nested.getEnumConstants()) ids.put(Integer.toString(enumId((Enum<?>)value)),((Enum<?>)value).name());
                        enums.put(nested.getSimpleName(),ids);
                        Map<String,Object> values=new TreeMap<>();
                        for(Object value:nested.getEnumConstants()) {
                            Map<String,Object> numbers=new TreeMap<>();
                            for(Field field:nested.getDeclaredFields()) if(!Modifier.isStatic(field.getModifiers())) {
                                field.setAccessible(true);Object number=field.get(value);
                                if(number instanceof Number) numbers.put(field.getName(),number);
                            }
                            values.put(Integer.toString(enumId((Enum<?>)value)),numbers);
                        }
                        enumNumbers.put(nested.getSimpleName(),values);
                    }
            if("net.minecraft.world.entity.animal.equine.AbstractChestedHorse".equals(methods.get("getDefaultDimensions")))
                dimensions.put("BABY_DIMENSIONS",dimension(type.getDimensions().withAttachments(
                    net.minecraft.world.entity.EntityAttachments.builder().attach(net.minecraft.world.entity.EntityAttachment.PASSENGER,
                        0,type.getHeight()+0.03125f,-0.3125f)).scale(0.5f)));
            Map<String,Object> entity = new LinkedHashMap<>();
            entity.put("protocol_id",net.minecraft.core.registries.BuiltInRegistries.ENTITY_TYPE.getId(type));
            entity.put("class",entityClass.getName());
            entity.put("width",type.getDimensions().width());
            entity.put("fire_immune",type.fireImmune());
            entity.put("height",type.getDimensions().height());
            entity.put("eye_height",type.getDimensions().eyeHeight());
            entity.put("dimension_constants",dimensions);
            entity.put("enum_ids",enums);
            entity.put("enum_numbers",enumNumbers);
            entity.put("hierarchy",hierarchy);
            entity.put("method_owners",methods);
            entity.put("dimensions",dimension(type.getDimensions()));
            Map<String,Object> attributes=new TreeMap<>();
            var supplier=net.minecraft.world.entity.ai.attributes.DefaultAttributes.getSupplier((net.minecraft.world.entity.EntityType)type);
            if(supplier!=null) {
                Field instances=net.minecraft.world.entity.ai.attributes.AttributeSupplier.class.getDeclaredField("instances");instances.setAccessible(true);
                for(var holder:((Map<net.minecraft.core.Holder<net.minecraft.world.entity.ai.attributes.Attribute>,?>)instances.get(supplier)).keySet())
                    attributes.put(holder.getRegisteredName(),Map.of("id",net.minecraft.core.registries.BuiltInRegistries.ATTRIBUTE.getId(holder.value()),"value",supplier.getValue(holder)));
            }
            entity.put("default_attributes",attributes);
            entity.put("accessors",accessors);
            entities.put(net.minecraft.core.registries.BuiltInRegistries.ENTITY_TYPE.getKey(type).toString(),entity);
        }
        Map<String,Object> serializers = new TreeMap<>();
        for (Field f:net.minecraft.network.syncher.EntityDataSerializers.class.getFields())
            if (f.getType()==net.minecraft.network.syncher.EntityDataSerializer.class)
                serializers.put(f.getName(),net.minecraft.network.syncher.EntityDataSerializers.getSerializedId((net.minecraft.network.syncher.EntityDataSerializer<?>)f.get(null)));
        Map<String,String> layers = new TreeMap<>();
        for (Field f:ModelLayers.class.getFields())
            if (f.getType()==ModelLayerLocation.class) layers.put(f.getName(),f.get(null).toString());
        List<String> simpleParticles = new ArrayList<>();
        for (var particle:net.minecraft.core.registries.BuiltInRegistries.PARTICLE_TYPE)
            if (particle instanceof net.minecraft.core.particles.SimpleParticleType)
                simpleParticles.add(net.minecraft.core.registries.BuiltInRegistries.PARTICLE_TYPE.getKey(particle).toString());
        Collections.sort(simpleParticles);
        Map<String,Object> rendererDefaults=rendererDefaults(roots);
        Field defaultSkinField=net.minecraft.client.resources.DefaultPlayerSkin.class.getDeclaredField("DEFAULT_SKINS");
        defaultSkinField.setAccessible(true);
        List<Object> defaultSkins=new ArrayList<>();
        for(var skin:(net.minecraft.world.entity.player.PlayerSkin[])defaultSkinField.get(null))
            defaultSkins.add(Map.of("texture",skin.body().texturePath().toString(),"model",skin.model().name()));
        Map<String,String> poses=new TreeMap<>();
        for(var pose:net.minecraft.world.entity.Pose.values()) poses.put(Integer.toString(pose.id()),pose.name());
        Map<String,Object> document = new LinkedHashMap<>();
        document.put("schema",2);
        document.put("game_version","26.1.2");
        document.put("client_sha256",args[1]);
        document.put("source_api","LayerDefinitions.createRoots -> LayerDefinition.bakeRoot -> ModelPart polygon vertices; analysis only");
        document.put("roots",roots);
        document.put("nonfinite_polygons",invalidFaces);
        document.put("entities",entities);
        document.put("serializers",serializers);
        document.put("model_layers",layers);
        document.put("simple_particles",simpleParticles);
        document.put("renderer_defaults",rendererDefaults);
        document.put("default_skins",defaultSkins);
        document.put("poses",poses);
        Map<String,Object> attributes=new TreeMap<>();
        for(var attribute:net.minecraft.core.registries.BuiltInRegistries.ATTRIBUTE) {
            Map<String,Object> descriptor=new LinkedHashMap<>();
            descriptor.put("id",net.minecraft.core.registries.BuiltInRegistries.ATTRIBUTE.getId(attribute));
            if(attribute instanceof net.minecraft.world.entity.ai.attributes.RangedAttribute ranged) {
                descriptor.put("min",ranged.getMinValue());descriptor.put("max",ranged.getMaxValue());
            }
            attributes.put(net.minecraft.core.registries.BuiltInRegistries.ATTRIBUTE.getKey(attribute).toString(),descriptor);
        }
        document.put("attributes",attributes);
        Files.writeString(Path.of(args[0]),new Gson().toJson(document));
        System.err.println("Exported "+roots.size()+" exact layer roots to "+args[0]);
    }
}
